//! Voice service (`OB-F13`): 8 STT models, 7 TTS models with `voiceAgent` resolution & speed cycle,
//! 25-voice WebRTC Live Voice SDP state machine, and custom OpenAI-compatible Voice Server settings
//! with `SealedKeyStore` API-key isolation and PCM16 WAV codecs.
//!
//! Mirrors `src/main/{voice-transcription-service,voice-speech-service,voice-server-service}.ts`,
//! `src/backend/agent/live-voice.ts`, and `packages/contracts/src/{ipc-voice,speech-text,voice-server}.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use chrono::{SecondsFormat, Utc};
use openbot_contracts::limits::{is_bounded_string, is_identifier, utf16_len};
use openbot_contracts::voice::{
    AUTOMATIC_VOICE_LANGUAGE, CodexLiveVoice, SPEECH_MAXIMUM_TEXT_LENGTH, SPEECH_SPEEDS,
    SpeechModelId, SpeechSettings, VOICE_API_KEY_MAX_LENGTH, VOICE_AUDIO_LIMITS,
    VOICE_LIVE_MAXIMUM_SDP_LENGTH, VOICE_TURN_PAUSES, VoiceLiveEvent, VoiceLiveSession,
    VoiceLiveStart, VoiceModelId, VoiceServerRoutes, VoiceServerSettings, VoiceSettings,
    default_voice_settings, model_language, next_speech_speed,
};
use openbot_storage::redact::register_secret_value;
use openbot_storage::{
    AgentRoster, ChannelStore, JsonPreferenceStore, OpenBotDatabase, SealedKeyStore,
    preference_files,
};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::error::{OpenBotRuntimeError, RuntimeResult};

pub const VOICE_SERVER_ADDRESS_MAX_LENGTH: usize = 2_048;
pub const VOICE_SERVER_NAME_MAX_LENGTH: usize = 256;
pub const VOICE_SERVER_MODELS_LIMIT: usize = 500;
pub const LIVE_VOICE_NAME_LIMIT: usize = 80;

pub const OPENAI_VOICES: [&str; 9] = [
    "alloy", "ash", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer",
];

pub const VOICE_SERVER_VOICE_PATHS: [&str; 2] = ["audio/voices", "audio/speech/voices"];

static TRANSCRIPTION_MODEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)whisper|transcri|stt|asr|parakeet|scribe|canary|sensevoice|speech-to-text")
        .expect("valid transcription model regex")
});

static SPEECH_MODEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)tts|kokoro|piper|orpheus|xtts|kitten|chatterbox|melo|zonos|supertonic|text-to-speech",
    )
    .expect("valid speech model regex")
});

static REALTIME_ONLY_MODEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|/)gpt-(?:[^/]*[-_.])?(?:realtime|live)(?:[-_.]|$)")
        .expect("valid realtime model regex")
});

static FENCED_BACKTICK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)```.*?(?:```|$)").expect("valid backtick fence regex"));
static FENCED_TILDE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)~~~.*?(?:~~~|$)").expect("valid tilde fence regex"));
static HTML_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>\n]+>").expect("valid html tag regex"));
static MD_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").expect("valid md image regex"));
static MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").expect("valid md link regex"));
static HTTP_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"https?://\S+").expect("valid url regex"));
static LIST_PREFIX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:>\s*)*(?:(?:#{1,6}|[-*+•◦▪‣⁃]|\d+[.)])\s+(?:\[[ xX]\]\s+)?)?")
        .expect("valid list prefix regex")
});
static INLINE_CODE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]*)`").expect("valid inline code regex"));
static STRONG_STAR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").expect("valid strong star regex"));
static STRONG_UNDER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"__(.+?)__").expect("valid strong under regex"));
static STRIKE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"~~(.+?)~~").expect("valid strike regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VoiceEvalMode {
    Stt,
    Tts,
    Live,
    Server,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEvalRequest {
    pub mode: String, // "stt" | "tts" | "live" | "server"
    pub locale: Option<String>,
    pub stt_model: Option<String>,
    pub audio_seconds: Option<u32>,
    pub wav_bytes: Option<usize>,
    pub tts_provider: Option<String>,
    pub tts_text: Option<String>,
    pub tts_speed: Option<f64>,
    pub tts_speed_steps: Option<usize>,
    pub agent_id: Option<String>,
    pub channel_id: Option<String>,
    pub live_voice: Option<String>,
    pub sdp: Option<String>,
    pub sdp_length: Option<usize>,
    pub voice_server_url: Option<String>,
    pub voice_server_api_key: Option<String>,
    pub voice_server_routes: Option<VoiceServerRoutes>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceEvalOutcome {
    pub mode: String,
    pub valid: bool,
    pub model_id: Option<String>,
    pub retired: Option<bool>,
    pub language: Option<String>,
    pub provider: Option<String>,
    pub speed: Option<f64>,
    pub next_speed: Option<f64>,
    pub speed_cycle: Option<Vec<f64>>,
    pub voice_agent_id: Option<String>,
    pub char_count: Option<usize>,
    pub live_voice: Option<String>,
    pub max_sdp_length: Option<usize>,
    pub session_id: Option<String>,
    pub settings: Option<VoiceServerSettings>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveVoiceSessionState {
    Active,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveVoiceSessionRecord {
    pub session_id: String,
    pub agent_id: String,
    pub voice: Option<CodexLiveVoice>,
    pub offer_sdp: String,
    pub answer_sdp: String,
    pub prompt: String,
    pub state: LiveVoiceSessionState,
    pub started_at: String,
    pub transcript: Vec<VoiceLiveEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredVoiceServerEntry {
    address: String,
    routes: VoiceServerRoutes,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredVoiceServer {
    server: Option<StoredVoiceServerEntry>,
}

pub struct VoiceService {
    database: OpenBotDatabase,
    data_dir: PathBuf,
    voice_preferences: JsonPreferenceStore<VoiceSettings>,
    speech_preferences: JsonPreferenceStore<SpeechSettings>,
    server_preferences: JsonPreferenceStore<StoredVoiceServer>,
    server_api_key: SealedKeyStore,
    live_sessions: Mutex<HashMap<String, LiveVoiceSessionRecord>>,
}

impl VoiceService {
    pub fn open(database: OpenBotDatabase, data_dir: impl AsRef<Path>) -> RuntimeResult<Arc<Self>> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let voice_preferences = JsonPreferenceStore::new(
            data_dir.join(preference_files::VOICE_PREFERENCE),
            VoiceSettings::default(),
        );
        let speech_preferences = JsonPreferenceStore::new(
            data_dir.join(preference_files::SPEECH_PREFERENCE),
            SpeechSettings::default(),
        );
        let server_preferences = JsonPreferenceStore::with_decoder(
            data_dir.join(preference_files::VOICE_SERVER),
            StoredVoiceServer::default(),
            decode_stored_voice_server,
        );
        let server_api_key =
            SealedKeyStore::new_passthrough(data_dir.join(preference_files::VOICE_SERVER_KEY));

        let _ = voice_preferences.load()?;
        let _ = speech_preferences.load()?;
        let _ = server_preferences.load()?;
        if let Some(key) = server_api_key.load()? {
            register_secret_value(&key, false);
        }

        Ok(Arc::new(Self {
            database,
            data_dir,
            voice_preferences,
            speech_preferences,
            server_preferences,
            server_api_key,
            live_sessions: Mutex::new(HashMap::new()),
        }))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn get_voice_settings(&self) -> VoiceSettings {
        self.voice_preferences.get()
    }

    pub fn save_voice_settings(&self, settings: VoiceSettings) -> RuntimeResult<VoiceSettings> {
        let normalized_lang = model_language(settings.model_id, settings.language.trim()).to_string();
        let next = VoiceSettings {
            model_id: settings.model_id,
            language: if normalized_lang.is_empty() {
                AUTOMATIC_VOICE_LANGUAGE.to_string()
            } else {
                normalized_lang
            },
        };
        Ok(self.voice_preferences.set(next)?)
    }

    pub fn get_speech_settings(&self) -> SpeechSettings {
        self.speech_preferences.get()
    }

    pub fn save_speech_settings(&self, mut settings: SpeechSettings) -> RuntimeResult<SpeechSettings> {
        settings.speed = validate_speech_speed(settings.speed)?;
        settings.turn_pause_ms = validate_turn_pause_ms(settings.turn_pause_ms)?;
        Ok(self.speech_preferences.set(settings)?)
    }

    pub fn eval(&self, req: VoiceEvalRequest) -> RuntimeResult<VoiceEvalOutcome> {
        let mode = match req.mode.trim().to_ascii_lowercase().as_str() {
            "stt" => VoiceEvalMode::Stt,
            "tts" => VoiceEvalMode::Tts,
            "live" => VoiceEvalMode::Live,
            "server" => VoiceEvalMode::Server,
            other => {
                return Err(OpenBotRuntimeError::Validation(format!(
                    "Unknown voice evaluation mode: {other}"
                )));
            }
        };
        match mode {
            VoiceEvalMode::Stt => self.eval_stt(req),
            VoiceEvalMode::Tts => self.eval_tts(req),
            VoiceEvalMode::Live => self.eval_live(req),
            VoiceEvalMode::Server => self.eval_server(req),
        }
    }

    fn eval_stt(&self, req: VoiceEvalRequest) -> RuntimeResult<VoiceEvalOutcome> {
        if let Some(secs) = req.audio_seconds
            && secs > VOICE_AUDIO_LIMITS.maximum_seconds
        {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Audio duration {secs}s exceeds maximum of {}s.",
                VOICE_AUDIO_LIMITS.maximum_seconds
            )));
        }
        if let Some(bytes) = req.wav_bytes
            && bytes > VOICE_AUDIO_LIMITS.maximum_wav_bytes
        {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "WAV size {bytes} bytes exceeds maximum of {} bytes.",
                VOICE_AUDIO_LIMITS.maximum_wav_bytes
            )));
        }

        let locale = req.locale.as_deref().unwrap_or("en");
        let defaults = default_voice_settings(locale);
        let model = match req.stt_model.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(raw) => parse_voice_model_id(raw).ok_or_else(|| {
                OpenBotRuntimeError::Validation(format!("Unknown STT voice model: {raw}"))
            })?,
            None => defaults.model_id,
        };

        let raw_lang = if req.stt_model.is_some() {
            req.locale
                .as_deref()
                .map(canonical_language_code)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| AUTOMATIC_VOICE_LANGUAGE.to_string())
        } else {
            defaults.language
        };
        let resolved_lang = model_language(model, &raw_lang).to_string();
        let provider = stt_provider_name(model);

        Ok(VoiceEvalOutcome {
            mode: "stt".to_string(),
            valid: true,
            model_id: Some(model.as_str().to_string()),
            retired: Some(model.is_retired()),
            language: Some(resolved_lang),
            provider: Some(provider.to_string()),
            speed: None,
            next_speed: None,
            speed_cycle: None,
            voice_agent_id: None,
            char_count: None,
            live_voice: None,
            max_sdp_length: None,
            session_id: None,
            settings: None,
        })
    }

    fn eval_tts(&self, req: VoiceEvalRequest) -> RuntimeResult<VoiceEvalOutcome> {
        let model = match req
            .tts_provider
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => parse_speech_model_id(raw).ok_or_else(|| {
                OpenBotRuntimeError::Validation(format!("Unknown TTS provider/model: {raw}"))
            })?,
            None => SpeechModelId::Supertonic3,
        };

        let char_count = if let Some(text) = req.tts_text.as_deref() {
            let count = utf16_len(text);
            if count > SPEECH_MAXIMUM_TEXT_LENGTH {
                return Err(OpenBotRuntimeError::LimitExceeded(format!(
                    "TTS text length {count} exceeds maximum of {SPEECH_MAXIMUM_TEXT_LENGTH} characters."
                )));
            }
            Some(count)
        } else {
            None
        };

        let initial_speed = validate_speech_speed(req.tts_speed.unwrap_or(1.0))?;
        let steps = req.tts_speed_steps.unwrap_or(1);
        let mut speed_cycle = Vec::with_capacity(steps);
        let mut current = initial_speed;
        for _ in 0..steps {
            current = next_speech_speed(current);
            speed_cycle.push(current);
        }
        let final_next_speed = if steps == 0 {
            next_speech_speed(initial_speed)
        } else {
            current
        };

        let voice_agent_id = if let Some(aid) =
            req.agent_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
        {
            Some(aid.to_string())
        } else if let Some(cid) =
            req.channel_id.as_deref().map(str::trim).filter(|s| !s.is_empty())
        {
            let channel = ChannelStore::new(self.database.clone()).get(cid)?;
            channel
                .draft
                .lead_agent_id
                .clone()
                .or_else(|| channel.draft.members.first().map(|m| m.agent_id.clone()))
        } else {
            None
        };

        Ok(VoiceEvalOutcome {
            mode: "tts".to_string(),
            valid: true,
            model_id: Some(model.as_str().to_string()),
            retired: None,
            language: req.locale,
            provider: Some(tts_provider_name(model).to_string()),
            speed: Some(initial_speed),
            next_speed: Some(final_next_speed),
            speed_cycle: Some(speed_cycle),
            voice_agent_id,
            char_count,
            live_voice: None,
            max_sdp_length: None,
            session_id: None,
            settings: None,
        })
    }
}

impl VoiceService {
    fn eval_live(&self, req: VoiceEvalRequest) -> RuntimeResult<VoiceEvalOutcome> {
        let parsed_voice = match req
            .live_voice
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(raw) => Some(parse_codex_live_voice(raw).ok_or_else(|| {
                OpenBotRuntimeError::Validation(format!("Unknown Codex live voice: {raw}"))
            })?),
            None => None,
        };

        let effective_sdp_len = req
            .sdp
            .as_deref()
            .map(utf16_len)
            .or(req.sdp_length);
        if let Some(len) = effective_sdp_len
            && len > VOICE_LIVE_MAXIMUM_SDP_LENGTH
        {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Live voice SDP length {len} exceeds maximum of {VOICE_LIVE_MAXIMUM_SDP_LENGTH}."
            )));
        }

        let session_id = match (
            req.agent_id.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            req.sdp.as_deref().filter(|s| !s.trim().is_empty()),
        ) {
            (Some(agent_id), Some(sdp)) => {
                let session = self.start_live_session(
                    VoiceLiveStart {
                        agent_id: agent_id.to_string(),
                        sdp: sdp.to_string(),
                    },
                    parsed_voice,
                )?;
                Some(session.session_id)
            }
            _ => None,
        };

        Ok(VoiceEvalOutcome {
            mode: "live".to_string(),
            valid: true,
            model_id: None,
            retired: None,
            language: req.locale,
            provider: Some("codex-live".to_string()),
            speed: None,
            next_speed: None,
            speed_cycle: None,
            voice_agent_id: req.agent_id,
            char_count: effective_sdp_len,
            live_voice: parsed_voice.map(|v| codex_live_voice_as_str(v).to_string()),
            max_sdp_length: Some(VOICE_LIVE_MAXIMUM_SDP_LENGTH),
            session_id,
            settings: None,
        })
    }

    fn eval_server(&self, req: VoiceEvalRequest) -> RuntimeResult<VoiceEvalOutcome> {
        let settings = if let Some(address) = req.voice_server_url {
            let routes = req.voice_server_routes.unwrap_or(VoiceServerRoutes {
                dictation: false,
                speech: false,
                transcription_model: None,
                speech_model: None,
                voice: None,
            });
            Some(self.save_voice_server_settings(address, req.voice_server_api_key, routes)?)
        } else {
            self.get_voice_server_settings()?
        };

        Ok(VoiceEvalOutcome {
            mode: "server".to_string(),
            valid: true,
            model_id: settings
                .as_ref()
                .and_then(|s| s.routes.transcription_model.clone().or_else(|| s.routes.speech_model.clone())),
            retired: None,
            language: req.locale,
            provider: Some("voice-server".to_string()),
            speed: None,
            next_speed: None,
            speed_cycle: None,
            voice_agent_id: req.agent_id,
            char_count: None,
            live_voice: None,
            max_sdp_length: None,
            session_id: None,
            settings,
        })
    }

    pub fn start_live_session(
        &self,
        start: VoiceLiveStart,
        voice: Option<CodexLiveVoice>,
    ) -> RuntimeResult<VoiceLiveSession> {
        let agent_id = start.agent_id.trim();
        if agent_id.is_empty() || !is_identifier(agent_id) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid agentId for live voice session.".to_string(),
            ));
        }
        if start.sdp.trim().is_empty() {
            return Err(OpenBotRuntimeError::Validation(
                "Live voice WebRTC SDP offer is required.".to_string(),
            ));
        }
        let sdp_len = utf16_len(&start.sdp);
        if sdp_len > VOICE_LIVE_MAXIMUM_SDP_LENGTH {
            return Err(OpenBotRuntimeError::LimitExceeded(format!(
                "Live voice SDP length {sdp_len} exceeds maximum of {VOICE_LIVE_MAXIMUM_SDP_LENGTH}."
            )));
        }

        let agent_name = AgentRoster::new(self.database.clone())
            .get_agent(agent_id)?
            .map(|a| a.name)
            .unwrap_or_else(|| agent_id.to_string());
        let prompt = live_voice_prompt(&agent_name);
        let session_id = format!("live-{}", Uuid::new_v4());
        let voice_label = voice.map(codex_live_voice_as_str).unwrap_or("juniper");
        let answer_sdp = build_live_voice_sdp_answer(&session_id, voice_label, &start.sdp);

        let record = LiveVoiceSessionRecord {
            session_id: session_id.clone(),
            agent_id: agent_id.to_string(),
            voice,
            offer_sdp: start.sdp,
            answer_sdp: answer_sdp.clone(),
            prompt,
            state: LiveVoiceSessionState::Active,
            started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            transcript: Vec::new(),
        };
        self.live_sessions.lock().insert(session_id.clone(), record);

        Ok(VoiceLiveSession {
            session_id,
            sdp: answer_sdp,
        })
    }

    pub fn stop_live_session(&self, session_id: &str) -> RuntimeResult<bool> {
        let trimmed = session_id.trim();
        if trimmed.is_empty() || !is_identifier(trimmed) {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid live voice sessionId.".to_string(),
            ));
        }
        let mut guard = self.live_sessions.lock();
        if let Some(mut session) = guard.remove(trimmed) {
            session.state = LiveVoiceSessionState::Stopped;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get_live_session(&self, session_id: &str) -> Option<LiveVoiceSessionRecord> {
        self.live_sessions.lock().get(session_id).cloned()
    }

    pub fn record_live_transcript(
        &self,
        session_id: &str,
        role: &str,
        text: &str,
        is_final: bool,
    ) -> RuntimeResult<VoiceLiveEvent> {
        let event = VoiceLiveEvent::Transcript {
            session_id: session_id.to_string(),
            role: role.to_string(),
            text: text.to_string(),
            is_final,
        };
        if !event.is_valid() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid live voice transcript event.".to_string(),
            ));
        }
        let mut guard = self.live_sessions.lock();
        let Some(session) = guard.get_mut(session_id) else {
            return Err(OpenBotRuntimeError::NotFound(format!(
                "Live voice session not found: {session_id}"
            )));
        };
        session.transcript.push(event.clone());
        Ok(event)
    }

    pub fn get_voice_server_settings(&self) -> RuntimeResult<Option<VoiceServerSettings>> {
        let stored = self.server_preferences.load()?;
        let has_api_key = self.server_api_key.load()?.is_some();
        Ok(stored.server.map(|entry| VoiceServerSettings {
            address: entry.address,
            has_api_key,
            routes: entry.routes,
        }))
    }

    pub fn save_voice_server_settings(
        &self,
        address: String,
        api_key: Option<String>,
        routes: VoiceServerRoutes,
    ) -> RuntimeResult<VoiceServerSettings> {
        let trimmed_address = address.trim().to_string();
        if trimmed_address.is_empty()
            || !is_bounded_string(&trimmed_address, VOICE_SERVER_ADDRESS_MAX_LENGTH)
        {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid voice server address.".to_string(),
            ));
        }
        let parsed_url = Url::parse(&trimmed_address).map_err(|_| {
            OpenBotRuntimeError::Validation("Voice server address must be a valid URL.".to_string())
        })?;
        if !matches!(parsed_url.scheme(), "http" | "https") || parsed_url.host_str().is_none() {
            return Err(OpenBotRuntimeError::Validation(
                "Voice server address must use http or https with a valid host.".to_string(),
            ));
        }
        if !routes.is_valid() {
            return Err(OpenBotRuntimeError::Validation(
                "Invalid voice server routes configuration.".to_string(),
            ));
        }

        let previous_key = self.server_api_key.get();
        let previous_server = self.server_preferences.get().server;
        let next_key: Option<String> = match api_key {
            Some(raw) => {
                let k = raw.trim();
                if k.is_empty() {
                    None
                } else {
                    if utf16_len(k) > VOICE_API_KEY_MAX_LENGTH {
                        return Err(OpenBotRuntimeError::LimitExceeded(format!(
                            "Voice server API key exceeds maximum length of {VOICE_API_KEY_MAX_LENGTH}."
                        )));
                    }
                    register_secret_value(k, false);
                    Some(k.to_string())
                }
            }
            None => match (&previous_server, &previous_key) {
                (Some(saved), Some(existing))
                    if same_server_origin(&saved.address, &trimmed_address) =>
                {
                    Some(existing.clone())
                }
                _ => None,
            },
        };

        self.server_api_key.set(next_key.as_deref())?;
        let save_res = self.server_preferences.set(StoredVoiceServer {
            server: Some(StoredVoiceServerEntry {
                address: trimmed_address.clone(),
                routes: routes.clone(),
            }),
        });
        if let Err(err) = save_res {
            let _ = self.server_api_key.set(previous_key.as_deref());
            return Err(err.into());
        }

        Ok(VoiceServerSettings {
            address: trimmed_address,
            has_api_key: next_key.is_some(),
            routes,
        })
    }

    pub fn clear_voice_server_settings(&self) -> RuntimeResult<()> {
        let previous_key = self.server_api_key.get();
        self.server_api_key.set(None)?;
        if let Err(err) = self.server_preferences.set(StoredVoiceServer { server: None }) {
            let _ = self.server_api_key.set(previous_key.as_deref());
            return Err(err.into());
        }
        Ok(())
    }
}

fn validate_speech_speed(speed: f64) -> RuntimeResult<f64> {
    if !speed.is_finite() {
        return Err(OpenBotRuntimeError::Validation(format!(
            "Unsupported TTS playback speed {speed}; allowed speeds are 0.75, 1.0, 1.25, 1.5, 2.0."
        )));
    }
    SPEECH_SPEEDS
        .iter()
        .copied()
        .find(|&allowed| (allowed - speed).abs() <= 1e-6)
        .ok_or_else(|| {
            OpenBotRuntimeError::Validation(format!(
                "Unsupported TTS playback speed {speed}; allowed speeds are 0.75, 1.0, 1.25, 1.5, 2.0."
            ))
        })
}

fn validate_turn_pause_ms(turn_pause_ms: u32) -> RuntimeResult<u32> {
    if VOICE_TURN_PAUSES.contains(&turn_pause_ms) {
        Ok(turn_pause_ms)
    } else {
        Err(OpenBotRuntimeError::Validation(format!(
            "Unsupported voice turn pause {turn_pause_ms}ms; allowed values are 1500, 2000, 3000, 5000."
        )))
    }
}

fn decode_stored_voice_server(parsed: &serde_json::Value) -> Option<StoredVoiceServer> {
    let obj = parsed.as_object()?;
    if obj.contains_key("apiKey") {
        return None;
    }
    if let Some(version) = obj.get("version").and_then(serde_json::Value::as_u64)
        && version != 1
    {
        return None;
    }
    let server_val = obj.get("server")?;
    if server_val.is_null() {
        return Some(StoredVoiceServer { server: None });
    }
    let server_obj = server_val.as_object()?;
    if server_obj.contains_key("apiKey") {
        return None;
    }
    let entry: StoredVoiceServerEntry = serde_json::from_value(server_val.clone()).ok()?;
    if !is_bounded_string(&entry.address, VOICE_SERVER_ADDRESS_MAX_LENGTH)
        || !entry.routes.is_valid()
    {
        return None;
    }
    Some(StoredVoiceServer {
        server: Some(entry),
    })
}

fn same_server_origin(left: &str, right: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(left), Url::parse(right)) else {
        return false;
    };
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn canonical_language_code(locale: &str) -> String {
    let lower = locale.trim().to_ascii_lowercase();
    if lower == AUTOMATIC_VOICE_LANGUAGE {
        return AUTOMATIC_VOICE_LANGUAGE.to_string();
    }
    let primary = lower.split(['-', '_']).next().unwrap_or("");
    match primary {
        "nb" | "nn" => "no".to_string(),
        "iw" => "he".to_string(),
        "jv" => "jw".to_string(),
        other => other.to_string(),
    }
}

fn stt_provider_name(model: VoiceModelId) -> &'static str {
    match model {
        VoiceModelId::ParakeetTdt06bV3
        | VoiceModelId::ParakeetUnifiedEn06b
        | VoiceModelId::Bark30Plus
        | VoiceModelId::WhisperTurbo
        | VoiceModelId::CohereTranscribe032026 => "local",
        VoiceModelId::ElevenLabsScribeV2Realtime => "elevenlabs",
        VoiceModelId::GroqWhisperLargeV3Turbo | VoiceModelId::GroqWhisperLargeV3 => "groq",
    }
}

fn tts_provider_name(model: SpeechModelId) -> &'static str {
    match model {
        SpeechModelId::Supertonic3 => "local",
        SpeechModelId::ElevenLabsFlashV25
        | SpeechModelId::ElevenLabsMultilingualV2
        | SpeechModelId::ElevenLabsV4Turbo
        | SpeechModelId::ElevenLabsV4
        | SpeechModelId::ElevenLabsV3 => "elevenlabs",
        SpeechModelId::XaiTts => "xai",
    }
}

pub fn parse_voice_model_id(raw: &str) -> Option<VoiceModelId> {
    VoiceModelId::ALL
        .iter()
        .copied()
        .find(|m| m.as_str() == raw)
}

pub fn parse_speech_model_id(raw: &str) -> Option<SpeechModelId> {
    SpeechModelId::ALL
        .iter()
        .copied()
        .find(|m| m.as_str() == raw)
}

pub fn parse_codex_live_voice(raw: &str) -> Option<CodexLiveVoice> {
    match raw {
        "juniper" => Some(CodexLiveVoice::Juniper),
        "maple" => Some(CodexLiveVoice::Maple),
        "spruce" => Some(CodexLiveVoice::Spruce),
        "ember" => Some(CodexLiveVoice::Ember),
        "vale" => Some(CodexLiveVoice::Vale),
        "breeze" => Some(CodexLiveVoice::Breeze),
        "arbor" => Some(CodexLiveVoice::Arbor),
        "sol" => Some(CodexLiveVoice::Sol),
        "cove" => Some(CodexLiveVoice::Cove),
        "aube" => Some(CodexLiveVoice::Aube),
        "bubbie" => Some(CodexLiveVoice::Bubbie),
        "haetsal" => Some(CodexLiveVoice::Haetsal),
        "hanul" => Some(CodexLiveVoice::Hanul),
        "himari" => Some(CodexLiveVoice::Himari),
        "leher" => Some(CodexLiveVoice::Leher),
        "miko" => Some(CodexLiveVoice::Miko),
        "neer" => Some(CodexLiveVoice::Neer),
        "porto" => Some(CodexLiveVoice::Porto),
        "rindo" => Some(CodexLiveVoice::Rindo),
        "rio" => Some(CodexLiveVoice::Rio),
        "rivage" => Some(CodexLiveVoice::Rivage),
        "selva" => Some(CodexLiveVoice::Selva),
        "sonna" => Some(CodexLiveVoice::Sonna),
        "tinta" => Some(CodexLiveVoice::Tinta),
        "viola" => Some(CodexLiveVoice::Viola),
        _ => None,
    }
}

pub const fn codex_live_voice_as_str(voice: CodexLiveVoice) -> &'static str {
    match voice {
        CodexLiveVoice::Juniper => "juniper",
        CodexLiveVoice::Maple => "maple",
        CodexLiveVoice::Spruce => "spruce",
        CodexLiveVoice::Ember => "ember",
        CodexLiveVoice::Vale => "vale",
        CodexLiveVoice::Breeze => "breeze",
        CodexLiveVoice::Arbor => "arbor",
        CodexLiveVoice::Sol => "sol",
        CodexLiveVoice::Cove => "cove",
        CodexLiveVoice::Aube => "aube",
        CodexLiveVoice::Bubbie => "bubbie",
        CodexLiveVoice::Haetsal => "haetsal",
        CodexLiveVoice::Hanul => "hanul",
        CodexLiveVoice::Himari => "himari",
        CodexLiveVoice::Leher => "leher",
        CodexLiveVoice::Miko => "miko",
        CodexLiveVoice::Neer => "neer",
        CodexLiveVoice::Porto => "porto",
        CodexLiveVoice::Rindo => "rindo",
        CodexLiveVoice::Rio => "rio",
        CodexLiveVoice::Rivage => "rivage",
        CodexLiveVoice::Selva => "selva",
        CodexLiveVoice::Sonna => "sonna",
        CodexLiveVoice::Tinta => "tinta",
        CodexLiveVoice::Viola => "viola",
    }
}

fn build_live_voice_sdp_answer(session_id: &str, voice_label: &str, offer_sdp: &str) -> String {
    let has_ice_ufrag = offer_sdp.lines().any(|l| l.starts_with("a=ice-ufrag:"));
    let setup = if offer_sdp.contains("a=setup:active") {
        "passive"
    } else {
        "active"
    };
    let mut lines = vec![
        "v=0".to_string(),
        format!("o=openbot-live {} 2 IN IP4 127.0.0.1", Utc::now().timestamp()),
        format!("s=OpenBot Live Voice ({session_id})"),
        "t=0 0".to_string(),
        "m=audio 9 UDP/TLS/RTP/SAVPF 111".to_string(),
        "c=IN IP4 0.0.0.0".to_string(),
        format!("a=setup:{setup}"),
        "a=mid:0".to_string(),
        "a=sendrecv".to_string(),
        "a=rtpmap:111 opus/48000/2".to_string(),
        format!("a=openbot-voice:{voice_label}"),
    ];
    if has_ice_ufrag {
        lines.push("a=ice-options:trickle".to_string());
    }
    lines.join("\r\n") + "\r\n"
}

/// Converts Markdown into clean spoken plain sentences (`packages/contracts/src/speech-text.ts`).
pub fn speech_text_from_markdown(markdown: &str) -> String {
    let stripped = FENCED_BACKTICK_RE.replace_all(markdown, "\n");
    let stripped = FENCED_TILDE_RE.replace_all(&stripped, "\n");
    let stripped = HTML_TAG_RE.replace_all(&stripped, " ");
    let stripped = MD_IMAGE_RE.replace_all(&stripped, " ");
    let stripped = MD_LINK_RE.replace_all(&stripped, "$1");
    let stripped = HTTP_URL_RE.replace_all(&stripped, " ");

    let mut sentences = Vec::new();
    for raw in stripped.split('\n') {
        if raw.trim_start().starts_with('|') {
            continue;
        }
        let line = LIST_PREFIX_RE.replace(raw, "");
        let line = INLINE_CODE_RE.replace_all(&line, "$1");
        let line = strip_leading_mention_at(&line);
        let line = STRONG_STAR_RE.replace_all(&line, "$1");
        let line = STRONG_UNDER_RE.replace_all(&line, "$1");
        let line = STRIKE_RE.replace_all(&line, "$1");
        let line = strip_single_emphasis(&line);
        let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !collapsed.chars().any(char::is_alphanumeric) {
            continue;
        }
        let ends_punct = collapsed.chars().last().is_some_and(|ch| {
            matches!(
                ch,
                '.' | '!' | '?' | '…' | '。' | '！' | '？' | ':' | ';' | ','
            )
        });
        if ends_punct {
            sentences.push(collapsed);
        } else {
            sentences.push(format!("{collapsed}."));
        }
    }
    sentences.join(" ")
}

fn strip_leading_mention_at(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (idx, &ch) in chars.iter().enumerate() {
        if ch == '@'
            && (idx == 0 || chars[idx - 1].is_whitespace() || chars[idx - 1] == '(')
            && chars.get(idx + 1).is_some_and(|c| c.is_alphanumeric())
        {
            continue;
        }
        out.push(ch);
    }
    out
}

fn strip_single_emphasis(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if (ch == '*' || ch == '_')
            && (i == 0 || !chars[i - 1].is_alphanumeric())
            && chars.get(i + 1).is_some_and(|c| !c.is_whitespace())
            && let Some(close_rel) = chars[i + 1..].iter().position(|&c| c == ch)
        {
            let close = i + 1 + close_rel;
            let inner_ok = close > i + 1 && !chars[close - 1].is_whitespace();
            let after_ok = close + 1 >= chars.len() || !chars[close + 1].is_alphanumeric();
            if inner_ok && after_ok {
                for &inner_ch in &chars[i + 1..close] {
                    out.push(inner_ch);
                }
                i = close + 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Generates the live-voice system rule for an agent (`src/backend/agent/live-voice.ts:L58-L74`).
pub fn live_voice_prompt(agent_name: &str) -> String {
    let cleaned: String = agent_name
        .chars()
        .map(|c| if c == '"' || c.is_whitespace() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let clipped: String = cleaned.chars().take(LIVE_VOICE_NAME_LIMIT).collect();
    let trimmed = clipped.trim();
    let identity_prefix = if trimmed.is_empty() {
        String::new()
    } else {
        format!(
            "You are the voice of my agent \"{trimmed}\". When I ask who you are, you are {trimmed}. \
             Never say that you are ChatGPT, Codex or another OpenAI assistant. "
        )
    };
    format!(
        "{identity_prefix}\
         From now on, give every request of mine to the agent, even simple questions and small talk. \
         You and the agent are one: speak as the agent. Never say that you pass, forward or hand over a request, \
         or that someone else answers it. \
         When I stop in the middle of a sentence to think, wait silently for the rest, \
         and then give the complete request to the agent in one piece. \
         Never answer from your own knowledge. Wait for the agent's result before you give an answer. \
         Say only what the result says, and add no facts of your own. \
         Always speak the language that I speak, also for your own short words such as greetings. \
         Keep that language when a short or unclear phrase sounds like a different one. \
         Also use the same regional variety of the language as I do, for example European or Brazilian Portuguese. \
         If I ask you to speak a different language or variety, use it. \
         Do not reply to this message."
    )
}

/// Classifies OpenAI-compatible `/models` payloads into STT and TTS lists (`packages/contracts/src/voice-server.ts:L13-L37`).
pub fn classify_voice_server_models(body: &serde_json::Value) -> Option<(Vec<String>, Vec<String>)> {
    let data = body.as_object()?.get("data")?.as_array()?;
    let mut transcription = Vec::new();
    let mut speech = Vec::new();
    for entry in data {
        let Some(obj) = entry.as_object() else {
            continue;
        };
        let Some(raw_id) = obj.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let id = raw_id.trim();
        if id.is_empty() || utf16_len(id) > VOICE_SERVER_NAME_MAX_LENGTH {
            continue;
        }
        let task = obj
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let target: Option<&mut Vec<String>> = if task == "automatic-speech-recognition" {
            Some(&mut transcription)
        } else if task == "text-to-speech" {
            Some(&mut speech)
        } else if REALTIME_ONLY_MODEL_RE.is_match(id) {
            None
        } else if TRANSCRIPTION_MODEL_RE.is_match(id) {
            Some(&mut transcription)
        } else if SPEECH_MODEL_RE.is_match(id) {
            Some(&mut speech)
        } else {
            None
        };
        if let Some(list) = target
            && list.len() < VOICE_SERVER_MODELS_LIMIT
            && !list.iter().any(|existing| existing == id)
        {
            list.push(id.to_string());
        }
    }
    Some((transcription, speech))
}

/// Extracts voice identifiers from OpenAI-compatible voice list responses (`voice-server.ts:L39-L60`).
pub fn extract_voice_server_voices(body: &serde_json::Value) -> Vec<String> {
    let list = if let Some(arr) = body.as_array() {
        arr.as_slice()
    } else if let Some(arr) = body
        .as_object()
        .and_then(|o| o.get("voices").or_else(|| o.get("data")))
        .and_then(serde_json::Value::as_array)
    {
        arr.as_slice()
    } else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in list {
        let raw = if let Some(s) = entry.as_str() {
            Some(s)
        } else if let Some(obj) = entry.as_object() {
            obj.get("voice_id")
                .or_else(|| obj.get("id"))
                .or_else(|| obj.get("name"))
                .and_then(serde_json::Value::as_str)
        } else {
            None
        };
        if let Some(trimmed) = raw.map(str::trim)
            && !trimmed.is_empty()
            && utf16_len(trimmed) <= VOICE_SERVER_NAME_MAX_LENGTH
            && !names.iter().any(|n| n == trimmed)
        {
            names.push(trimmed.to_string());
            if names.len() >= VOICE_SERVER_MODELS_LIMIT {
                break;
            }
        }
    }
    names
}

/// Encodes mono 16-bit PCM into a RIFF/WAVE buffer (`voice-server.ts:L141-L162`).
pub fn encode_voice_server_wav(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    let pcm_len = pcm.len() as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32.saturating_add(pcm_len)).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate.saturating_mul(2)).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&pcm_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

/// Decodes PCM16 or IEEE Float32 RIFF/WAVE into mono PCM16 + sample rate (`voice-server.ts:L86-L139`).
pub fn decode_voice_server_wav(wav: &[u8]) -> Option<(Vec<u8>, u32)> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut offset = 12usize;
    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let declared = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into().ok()?) as usize;
        let start = offset + 8;
        if id == b"fmt " && declared >= 16 && start + 16 <= wav.len() {
            let raw_code = u16::from_le_bytes(wav[start..start + 2].try_into().ok()?);
            let code = if raw_code == 0xfffe && declared >= 26 && start + 26 <= wav.len() {
                u16::from_le_bytes(wav[start + 24..start + 26].try_into().ok()?)
            } else {
                raw_code
            };
            let channels = u16::from_le_bytes(wav[start + 2..start + 4].try_into().ok()?);
            let sample_rate = u32::from_le_bytes(wav[start + 4..start + 8].try_into().ok()?);
            let bits = u16::from_le_bytes(wav[start + 14..start + 16].try_into().ok()?);
            format = Some((code, channels, sample_rate, bits));
        } else if id == b"data" {
            let (code, channels, sample_rate, bits) = format?;
            let is_pcm16 = code == 1 && bits == 16;
            let is_float32 = code == 3 && bits == 32;
            if (!is_pcm16 && !is_float32)
                || !(1..=8).contains(&channels)
                || !(8_000..=192_000).contains(&sample_rate)
            {
                return None;
            }
            let end = if declared == 0 || start.saturating_add(declared) > wav.len() {
                wav.len()
            } else {
                start + declared
            };
            let bytes_per_sample = usize::from(bits / 8);
            let frame_bytes = bytes_per_sample * usize::from(channels);
            if frame_bytes == 0 || end < start {
                return None;
            }
            let frames = (end - start) / frame_bytes;
            let mut out = Vec::with_capacity(frames * 2);
            for frame in 0..frames {
                let mut sum = 0.0f64;
                for ch in 0..usize::from(channels) {
                    let at = start + frame * frame_bytes + ch * bytes_per_sample;
                    let sample = if is_pcm16 {
                        f64::from(i16::from_le_bytes(wav[at..at + 2].try_into().ok()?)) / 32_768.0
                    } else {
                        f64::from(f32::from_le_bytes(wav[at..at + 4].try_into().ok()?))
                    };
                    sum += sample;
                }
                let mono = (sum / f64::from(channels)).clamp(-1.0, 1.0);
                let sample_i16 = (mono * 32_767.0).round() as i16;
                out.extend_from_slice(&sample_i16.to_le_bytes());
            }
            return Some((out, sample_rate));
        }
        let step = declared.saturating_add(declared % 2);
        offset = start.checked_add(step)?;
    }
    None
}
