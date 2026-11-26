//! Voice contracts: 8 STT models (`VOICE_MODEL_IDS`), 7 TTS models (`SPEECH_MODEL_IDS`),
//! 25 WebRTC Live Voice identities (`CODEX_LIVE_VOICES`), locale routing, speed cycling, and custom voice server.
//! Mirrors `packages/contracts/src/ipc-voice.ts`.

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::{ContractError, is_bounded_string};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceAudioLimits {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub maximum_seconds: u32,
    pub maximum_wav_bytes: usize,
    pub maximum_recording_bytes: usize,
}

pub const VOICE_AUDIO_LIMITS: VoiceAudioLimits = VoiceAudioLimits {
    sample_rate: 16_000,
    channels: 1,
    bits_per_sample: 16,
    maximum_seconds: 600,
    maximum_wav_bytes: 19_200_044,
    maximum_recording_bytes: 1_048_576,
};

pub const VOICE_API_KEY_MAX_LENGTH: usize = 512;
pub const VOICE_STREAM_MAXIMUM_CHUNK_BYTES: usize = 160_000;
pub const SPEECH_MAXIMUM_TEXT_LENGTH: usize = 1_000;
pub const VOICE_LIVE_MAXIMUM_SDP_LENGTH: usize = 65_536;
pub const SUPERTONIC_3_BYTES: u64 = 145_296_838;
pub const AUTOMATIC_VOICE_LANGUAGE: &str = "automatic";

/// All 8 STT models (`VOICE_MODEL_IDS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum VoiceModelId {
    #[default]
    #[serde(rename = "parakeet-tdt-0.6b-v3")]
    ParakeetTdt06bV3,
    #[serde(rename = "parakeet-unified-en-0.6b")]
    ParakeetUnifiedEn06b,
    #[serde(rename = "bark-3.0-plus")]
    Bark30Plus,
    #[serde(rename = "whisper-turbo")]
    WhisperTurbo,
    #[serde(rename = "cohere-transcribe-03-2026")]
    CohereTranscribe032026,
    #[serde(rename = "elevenlabs-scribe-v2-realtime")]
    ElevenLabsScribeV2Realtime,
    #[serde(rename = "groq-whisper-large-v3-turbo")]
    GroqWhisperLargeV3Turbo,
    #[serde(rename = "groq-whisper-large-v3")]
    GroqWhisperLargeV3,
}

impl VoiceModelId {
    pub const ALL: [Self; 8] = [
        Self::ParakeetTdt06bV3,
        Self::ParakeetUnifiedEn06b,
        Self::Bark30Plus,
        Self::WhisperTurbo,
        Self::CohereTranscribe032026,
        Self::ElevenLabsScribeV2Realtime,
        Self::GroqWhisperLargeV3Turbo,
        Self::GroqWhisperLargeV3,
    ];

    pub const LOCAL: [Self; 5] = [
        Self::ParakeetTdt06bV3,
        Self::ParakeetUnifiedEn06b,
        Self::Bark30Plus,
        Self::WhisperTurbo,
        Self::CohereTranscribe032026,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParakeetTdt06bV3 => "parakeet-tdt-0.6b-v3",
            Self::ParakeetUnifiedEn06b => "parakeet-unified-en-0.6b",
            Self::Bark30Plus => "bark-3.0-plus",
            Self::WhisperTurbo => "whisper-turbo",
            Self::CohereTranscribe032026 => "cohere-transcribe-03-2026",
            Self::ElevenLabsScribeV2Realtime => "elevenlabs-scribe-v2-realtime",
            Self::GroqWhisperLargeV3Turbo => "groq-whisper-large-v3-turbo",
            Self::GroqWhisperLargeV3 => "groq-whisper-large-v3",
        }
    }

    pub const fn local_bytes(self) -> Option<u64> {
        match self {
            Self::ParakeetTdt06bV3 => Some(670_478_772),
            Self::ParakeetUnifiedEn06b => Some(663_043_117),
            Self::Bark30Plus => Some(684_624_267),
            Self::WhisperTurbo => Some(1_311_742_559),
            Self::CohereTranscribe032026 => Some(2_888_052_036),
            _ => None,
        }
    }

    pub const fn is_retired(self) -> bool {
        matches!(self, Self::Bark30Plus)
    }

    pub const fn live_preview(self) -> bool {
        matches!(
            self,
            Self::ParakeetTdt06bV3
                | Self::ParakeetUnifiedEn06b
                | Self::Bark30Plus
                | Self::ElevenLabsScribeV2Realtime
        )
    }
}

pub const DEFAULT_VOICE_MODEL_ID: VoiceModelId = VoiceModelId::ParakeetTdt06bV3;

pub const VOICE_MODEL_IDS: [&str; 8] = [
    "parakeet-tdt-0.6b-v3",
    "parakeet-unified-en-0.6b",
    "bark-3.0-plus",
    "whisper-turbo",
    "cohere-transcribe-03-2026",
    "elevenlabs-scribe-v2-realtime",
    "groq-whisper-large-v3-turbo",
    "groq-whisper-large-v3",
];

pub const COHERE_TRANSCRIBE_LANGUAGES: [&str; 14] = [
    "ar", "de", "el", "en", "es", "fr", "it", "ja", "ko", "nl", "pl", "pt", "vi", "zh",
];

pub const PARAKEET_TDT_LANGUAGES: [&str; 25] = [
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv",
    "mt", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
];

pub const WHISPER_LANGUAGES: [&str; 74] = [
    "af", "am", "ar", "az", "be", "bg", "bn", "bs", "ca", "cs", "cy", "da", "de", "el", "en",
    "es", "et", "eu", "fa", "fi", "fr", "gl", "gu", "he", "hi", "hr", "hu", "hy", "id", "is",
    "it", "ja", "jw", "ka", "kk", "km", "kn", "ko", "lo", "lt", "lv", "mk", "ml", "mn", "mr",
    "ms", "mt", "my", "ne", "nl", "no", "pa", "pl", "pt", "ro", "ru", "si", "sk", "sl", "sq",
    "sr", "su", "sv", "sw", "ta", "te", "th", "tr", "uk", "ur", "uz", "vi", "yue", "zh",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceSettings {
    pub model_id: VoiceModelId,
    pub language: String,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            model_id: DEFAULT_VOICE_MODEL_ID,
            language: AUTOMATIC_VOICE_LANGUAGE.to_string(),
        }
    }
}

pub fn default_voice_settings(locale: &str) -> VoiceSettings {
    let lower = locale.to_ascii_lowercase();
    let code = lower.split(['-', '_']).next().unwrap_or("");
    let canonical = match code {
        "nb" | "nn" => "no",
        "iw" => "he",
        "jv" => "jw",
        other => other,
    };
    if !WHISPER_LANGUAGES.contains(&canonical) || PARAKEET_TDT_LANGUAGES.contains(&canonical) {
        VoiceSettings::default()
    } else {
        VoiceSettings {
            model_id: VoiceModelId::WhisperTurbo,
            language: AUTOMATIC_VOICE_LANGUAGE.to_string(),
        }
    }
}

pub fn model_language<'a>(model_id: VoiceModelId, language: &'a str) -> &'a str {
    if model_id == VoiceModelId::CohereTranscribe032026 {
        if COHERE_TRANSCRIBE_LANGUAGES.contains(&language) {
            language
        } else {
            "en"
        }
    } else {
        language
    }
}

/// All 7 TTS models (`SPEECH_MODEL_IDS`: 1 local `supertonic-3` + 6 cloud).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SpeechModelId {
    #[default]
    #[serde(rename = "supertonic-3")]
    Supertonic3,
    #[serde(rename = "elevenlabs-flash-v2.5")]
    ElevenLabsFlashV25,
    #[serde(rename = "elevenlabs-multilingual-v2")]
    ElevenLabsMultilingualV2,
    #[serde(rename = "elevenlabs-v4-turbo")]
    ElevenLabsV4Turbo,
    #[serde(rename = "elevenlabs-v4")]
    ElevenLabsV4,
    #[serde(rename = "elevenlabs-v3")]
    ElevenLabsV3,
    #[serde(rename = "xai-tts")]
    XaiTts,
}

impl SpeechModelId {
    pub const ALL: [Self; 7] = [
        Self::Supertonic3,
        Self::ElevenLabsFlashV25,
        Self::ElevenLabsMultilingualV2,
        Self::ElevenLabsV4Turbo,
        Self::ElevenLabsV4,
        Self::ElevenLabsV3,
        Self::XaiTts,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supertonic3 => "supertonic-3",
            Self::ElevenLabsFlashV25 => "elevenlabs-flash-v2.5",
            Self::ElevenLabsMultilingualV2 => "elevenlabs-multilingual-v2",
            Self::ElevenLabsV4Turbo => "elevenlabs-v4-turbo",
            Self::ElevenLabsV4 => "elevenlabs-v4",
            Self::ElevenLabsV3 => "elevenlabs-v3",
            Self::XaiTts => "xai-tts",
        }
    }
}

pub const SPEECH_MODEL_IDS: [&str; 7] = [
    "supertonic-3",
    "elevenlabs-flash-v2.5",
    "elevenlabs-multilingual-v2",
    "elevenlabs-v4-turbo",
    "elevenlabs-v4",
    "elevenlabs-v3",
    "xai-tts",
];

pub const SPEECH_VOICES: [&str; 10] = ["F1", "F2", "F3", "F4", "F5", "M1", "M2", "M3", "M4", "M5"];
pub const ELEVENLABS_SPEECH_VOICES: [&str; 6] =
    ["alice", "chris", "eric", "george", "jessica", "sarah"];
pub const XAI_SPEECH_VOICES: [&str; 5] = ["eve", "ara", "leo", "rex", "sal"];
pub const SPEECH_LANGUAGES: [&str; 31] = [
    "ar", "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hi", "hr", "hu", "id",
    "it", "ja", "ko", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk",
    "vi",
];

pub const SPEECH_SPEEDS: [f64; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];
const SPEECH_SPEED_CYCLE: [f64; 5] = [1.0, 1.25, 1.5, 2.0, 0.75];
pub const VOICE_TURN_PAUSES: [u32; 4] = [1_500, 2_000, 3_000, 5_000];

pub fn next_speech_speed(speed: f64) -> f64 {
    if let Some(idx) = SPEECH_SPEED_CYCLE
        .iter()
        .position(|&s| (s - speed).abs() < f64::EPSILON)
    {
        SPEECH_SPEED_CYCLE[(idx + 1) % SPEECH_SPEED_CYCLE.len()]
    } else {
        1.0
    }
}

/// All 25 WebRTC Live Voice identities (`CODEX_LIVE_VOICES`, `ipc-voice.ts:L523-L550`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexLiveVoice {
    Juniper,
    Maple,
    Spruce,
    Ember,
    Vale,
    Breeze,
    Arbor,
    Sol,
    Cove,
    Aube,
    Bubbie,
    Haetsal,
    Hanul,
    Himari,
    Leher,
    Miko,
    Neer,
    Porto,
    Rindo,
    Rio,
    Rivage,
    Selva,
    Sonna,
    Tinta,
    Viola,
}

pub const CODEX_LIVE_VOICES: [&str; 25] = [
    "juniper", "maple", "spruce", "ember", "vale", "breeze", "arbor", "sol", "cove", "aube",
    "bubbie", "haetsal", "hanul", "himari", "leher", "miko", "neer", "porto", "rindo", "rio",
    "rivage", "selva", "sonna", "tinta", "viola",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSettings {
    pub read_replies: bool,
    pub model_id: SpeechModelId,
    pub voice: String,
    pub eleven_labs_voice: String,
    pub xai_voice: String,
    pub speed: f64,
    pub codex_live: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub codex_live_voice: Option<CodexLiveVoice>,
    pub turn_pause_ms: u32,
}

impl Default for SpeechSettings {
    fn default() -> Self {
        Self {
            read_replies: false,
            model_id: SpeechModelId::Supertonic3,
            voice: "F1".to_string(),
            eleven_labs_voice: "alice".to_string(),
            xai_voice: "eve".to_string(),
            speed: 1.0,
            codex_live: false,
            codex_live_voice: None,
            turn_pause_ms: VOICE_TURN_PAUSES[0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfig {
    pub voice: VoiceSettings,
    pub speech: SpeechSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<VoiceServerSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLiveStart {
    pub agent_id: String,
    pub sdp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLiveSession {
    pub session_id: String,
    pub sdp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceLiveStop {
    pub session_id: String,
}

fn deserialize_voice_live_role<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let role = String::deserialize(deserializer)?;
    if !matches!(role.as_str(), "user" | "assistant") {
        return Err(serde::de::Error::custom(
            "VoiceLiveEvent.Transcript.role must be 'user' or 'assistant'",
        ));
    }
    Ok(role)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum VoiceLiveEvent {
    Transcript {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(deserialize_with = "deserialize_voice_live_role")]
        role: String,
        text: String,
        #[serde(rename = "final")]
        is_final: bool,
    },
    Ended {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(deserialize_with = "Option::deserialize")]
        error: Option<String>,
    },
}

impl VoiceLiveEvent {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Transcript {
                session_id,
                role,
                text,
                ..
            } => {
                !session_id.is_empty()
                    && is_bounded_string(session_id, 128)
                    && matches!(role.as_str(), "user" | "assistant")
                    && is_bounded_string(text, 20_000)
            }
            Self::Ended { session_id, error } => {
                !session_id.is_empty()
                    && is_bounded_string(session_id, 128)
                    && error.as_deref().is_none_or(|e| is_bounded_string(e, 2_000))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServerRoutes {
    pub dictation: bool,
    pub speech: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub transcription_model: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub speech_model: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub voice: Option<String>,
}

impl VoiceServerRoutes {
    pub fn is_valid(&self) -> bool {
        let opt_ok = |v: Option<&str>| v.is_none_or(|s| !s.is_empty() && is_bounded_string(s, 256));
        opt_ok(self.transcription_model.as_deref())
            && opt_ok(self.speech_model.as_deref())
            && opt_ok(self.voice.as_deref())
            && (!self.dictation || self.transcription_model.is_some())
            && (!self.speech || (self.speech_model.is_some() && self.voice.is_some()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServerSettings {
    pub address: String,
    pub has_api_key: bool,
    pub routes: VoiceServerRoutes,
}

impl VoiceServerSettings {
    pub fn is_valid(&self) -> bool {
        is_bounded_string(&self.address, 2_048) && self.routes.is_valid()
    }
}

impl<'de> Deserialize<'de> for VoiceServerSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SettingsVisitor;

        impl<'de> Visitor<'de> for SettingsVisitor {
            type Value = VoiceServerSettings;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("VoiceServerSettings object without apiKey")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut address: Option<String> = None;
                let mut has_api_key: Option<bool> = None;
                let mut routes: Option<VoiceServerRoutes> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "apiKey" => {
                            return Err(serde::de::Error::custom(
                                "VoiceServerSettings must omit apiKey on read.",
                            ));
                        }
                        "address" => {
                            if address.is_some() {
                                return Err(serde::de::Error::duplicate_field("address"));
                            }
                            address = Some(map.next_value()?);
                        }
                        "hasApiKey" => {
                            if has_api_key.is_some() {
                                return Err(serde::de::Error::duplicate_field("hasApiKey"));
                            }
                            has_api_key = Some(map.next_value()?);
                        }
                        "routes" => {
                            if routes.is_some() {
                                return Err(serde::de::Error::duplicate_field("routes"));
                            }
                            routes = Some(map.next_value()?);
                        }
                        _ => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let parsed = VoiceServerSettings {
                    address: address.ok_or_else(|| serde::de::Error::missing_field("address"))?,
                    has_api_key: has_api_key
                        .ok_or_else(|| serde::de::Error::missing_field("hasApiKey"))?,
                    routes: routes.ok_or_else(|| serde::de::Error::missing_field("routes"))?,
                };
                if !parsed.is_valid() {
                    return Err(serde::de::Error::custom("Invalid voice server settings."));
                }
                Ok(parsed)
            }
        }

        deserializer.deserialize_map(SettingsVisitor)
    }
}

/// Strictly rejects `"apiKey"` on read (`ipc-voice.ts:isVoiceServerSettings`, `L705-L713`).
pub fn decode_voice_server_settings(value: &serde_json::Value) -> Result<VoiceServerSettings, ContractError> {
    serde_json::from_value(value.clone()).map_err(|e| ContractError::InvalidValue(e.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceServerConnection {
    pub address: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceServerModels {
    pub transcription: Vec<String>,
    pub speech: Vec<String>,
    pub voices: Vec<String>,
}

impl VoiceServerModels {
    pub fn is_valid(&self) -> bool {
        let valid_list = |list: &[String]| {
            list.len() <= 500
                && list
                    .iter()
                    .all(|s| !s.is_empty() && is_bounded_string(s, 256))
        };
        valid_list(&self.transcription) && valid_list(&self.speech) && valid_list(&self.voices)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VoiceServerProbeField {
    Address,
    ApiKey,
}

/// Boolean-discriminated probe result (`ipc-voice.ts:L674-L678, L715-L730`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceServerProbe {
    Ok {
        models: VoiceServerModels,
    },
    Failed {
        field: VoiceServerProbeField,
        message: String,
    },
}

impl VoiceServerProbe {
    pub fn is_valid(&self) -> bool {
        match self {
            Self::Ok { models } => models.is_valid(),
            Self::Failed { message, .. } => is_bounded_string(message, 2_000),
        }
    }
}

impl Serialize for VoiceServerProbe {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Ok { models } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("ok", &true)?;
                map.serialize_entry("models", models)?;
                map.end()
            }
            Self::Failed { field, message } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("ok", &false)?;
                map.serialize_entry("field", field)?;
                map.serialize_entry("message", message)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for VoiceServerProbe {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = serde_json::Value::deserialize(deserializer)?;
        let Some(obj) = val.as_object() else {
            return Err(serde::de::Error::custom("VoiceServerProbe must be an object"));
        };
        match obj.get("ok").and_then(serde_json::Value::as_bool) {
            Some(true) => {
                let models_val = obj
                    .get("models")
                    .ok_or_else(|| serde::de::Error::missing_field("models"))?;
                let models: VoiceServerModels =
                    serde_json::from_value(models_val.clone()).map_err(serde::de::Error::custom)?;
                if !models.is_valid() {
                    return Err(serde::de::Error::custom("Invalid VoiceServerModels"));
                }
                Ok(Self::Ok { models })
            }
            Some(false) => {
                let field_val = obj
                    .get("field")
                    .ok_or_else(|| serde::de::Error::missing_field("field"))?;
                let field: VoiceServerProbeField =
                    serde_json::from_value(field_val.clone()).map_err(serde::de::Error::custom)?;
                let message = obj
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| serde::de::Error::missing_field("message"))?
                    .to_string();
                if !is_bounded_string(&message, 2_000) {
                    return Err(serde::de::Error::custom("VoiceServerProbe message too long"));
                }
                Ok(Self::Failed { field, message })
            }
            None => Err(serde::de::Error::custom("VoiceServerProbe.ok must be a boolean")),
        }
    }
}
