//! Auto-generated module tree

pub mod backend;
pub use backend::AudioPlaybackEngine;
pub mod recorder;
pub use recorder::{enumerate_audio_devices, RecordedVoiceNote, VoiceRecorder};
pub mod wav;
pub use wav::{
    compute_pcm_rms_level, decode_wav_pcm, encode_wav_i16, waveform_from_pcm_i16, DecodedAudioPcm,
};
pub mod synth;
pub use synth::{
    midi_note_to_hz, parse_chord_symbol_to_midi, slice_pcm_to_wav_from_ms,
    slice_wav_from_offset_ms, synthesize_music_track_wav, SynthAdsr, SynthChord, SynthInstrument,
    SynthLyricLine, SynthNote, SynthTrackInput, SynthWaveformKind, SynthesizedTrackAudio,
    DEFAULT_SYNTH_SAMPLE_RATE, MAX_SYNTH_BPM, MAX_SYNTH_DURATION_SECS, MAX_SYNTH_SAMPLE_RATE,
    MIN_SYNTH_BPM, MIN_SYNTH_SAMPLE_RATE,
};
pub mod music_player;
pub use music_player::{
    find_lyric_index_at, MusicPlaybackState, MusicRepeatMode, MusicTrackEntry,
    MusicTransportEngine, MusicTransportSnapshot,
};
pub mod media_audio;
pub use media_audio::MediaAudioAudio;
pub mod media_audio_capture;
pub use media_audio_capture::MediaAudioCaptureAudio;
pub mod media_audio_capture_common;
pub use media_audio_capture_common::MediaAudioCaptureCommonAudio;
pub mod media_audio_edit;
pub use media_audio_edit::MediaAudioEditAudio;
pub mod media_audio_ffmpeg_loader;
pub use media_audio_ffmpeg_loader::MediaAudioFfmpegLoaderAudio;
pub mod media_audio_loader;
pub use media_audio_loader::MediaAudioLoaderAudio;
pub mod media_audio_loaders;
pub use media_audio_loaders::MediaAudioLoadersAudio;
pub mod media_audio_local_cache;
pub use media_audio_local_cache::MediaAudioLocalCacheAudio;
pub mod media_audio_track;
pub use media_audio_track::MediaAudioTrackAudio;
pub mod media_child_ffmpeg_loader;
pub use media_child_ffmpeg_loader::MediaChildFfmpegLoaderAudio;
