//! Cross-platform, headless-safe, seekable music transport engine for Apple Music Hub.
//!
//! Maintains an authoritative deterministic transport clock (`tick_ms`, `seek_ms`, queue,
//! repeat, seeded Fisher-Yates shuffle, volume, synced lyrics) on every platform and
//! optionally mirrors playback to `AudioPlaybackEngine` on macOS as best-effort.

use super::backend::AudioPlaybackEngine;
use super::synth::{
    slice_pcm_to_wav_from_ms, slice_wav_from_offset_ms, SynthLyricLine, SynthesizedTrackAudio,
    DEFAULT_SYNTH_SAMPLE_RATE,
};
use super::wav::{decode_wav_pcm, waveform_from_pcm_i16};
use serde::{Deserialize, Serialize};

const DEFAULT_SHUFFLE_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MusicRepeatMode {
    #[default]
    Off,
    One,
    All,
}

impl MusicRepeatMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MusicPlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicTrackEntry {
    pub track_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    #[serde(default)]
    pub pcm_samples: Vec<i16>,
    #[serde(default)]
    pub wav_bytes: Vec<u8>,
    #[serde(default)]
    pub waveform_bars: Vec<u8>,
    #[serde(default)]
    pub waveform_5bit: Vec<u8>,
    #[serde(default)]
    pub lyrics: Vec<SynthLyricLine>,
}

impl MusicTrackEntry {
    pub fn from_synthesized(
        track_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        audio: SynthesizedTrackAudio,
        lyrics: Vec<SynthLyricLine>,
    ) -> Self {
        Self {
            track_id: track_id.into(),
            creation_id: None,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms: audio.duration_ms,
            sample_rate: audio.sample_rate,
            channels: audio.channels,
            pcm_samples: audio.pcm_samples,
            wav_bytes: audio.wav_bytes,
            waveform_bars: audio.waveform_bars,
            waveform_5bit: audio.waveform_5bit,
            lyrics,
        }
    }

    pub fn from_wav_bytes(
        track_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        wav_bytes: Vec<u8>,
        lyrics: Vec<SynthLyricLine>,
    ) -> Result<Self, String> {
        let mut decoded = decode_wav_pcm(&wav_bytes).map_err(|e| e.to_string())?;
        if decoded.sample_rate == 0 {
            return Err("WAV sample_rate must be non-zero".to_string());
        }
        if decoded.channels != 1 && decoded.channels != 2 {
            return Err(format!(
                "WAV channels must be 1 (mono) or 2 (stereo), got {}",
                decoded.channels
            ));
        }
        let ch = usize::from(decoded.channels);
        let aligned_len = (decoded.samples.len() / ch) * ch;
        decoded.samples.truncate(aligned_len);
        let total_frames = aligned_len / ch;
        let duration_ms = ((total_frames as u128 * 1000) / u128::from(decoded.sample_rate)) as u64;

        let mono: Vec<i16> = if decoded.channels == 2 {
            decoded
                .samples
                .chunks_exact(2)
                .map(|pair| ((i32::from(pair[0]) + i32::from(pair[1])) / 2) as i16)
                .collect()
        } else {
            decoded.samples.clone()
        };
        let (waveform_bars, waveform_5bit) = waveform_from_pcm_i16(&mono);
        Ok(Self {
            track_id: track_id.into(),
            creation_id: None,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms,
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            pcm_samples: decoded.samples,
            wav_bytes,
            waveform_bars,
            waveform_5bit,
            lyrics,
        })
    }

    pub fn clock_only(
        track_id: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        duration_ms: u64,
        lyrics: Vec<SynthLyricLine>,
    ) -> Self {
        Self {
            track_id: track_id.into(),
            creation_id: None,
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration_ms,
            sample_rate: DEFAULT_SYNTH_SAMPLE_RATE,
            channels: 1,
            pcm_samples: Vec::new(),
            wav_bytes: Vec::new(),
            waveform_bars: Vec::new(),
            waveform_5bit: Vec::new(),
            lyrics,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicTransportSnapshot {
    pub state: MusicPlaybackState,
    pub current_index: Option<usize>,
    pub current_track_id: Option<String>,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub progress_ratio: f32,
    pub volume: f32,
    pub repeat_mode: MusicRepeatMode,
    pub shuffle: bool,
    pub queue_len: usize,
    pub active_lyric_index: Option<usize>,
}

/// Finds the active lyric index at `position_ms`, handling both sorted and unsorted lyric lists.
pub fn find_lyric_index_at(lyrics: &[SynthLyricLine], position_ms: u64) -> Option<usize> {
    let mut best: Option<(usize, u32)> = None;
    for (idx, line) in lyrics.iter().enumerate() {
        if u64::from(line.timestamp_ms) <= position_ms {
            match best {
                None => best = Some((idx, line.timestamp_ms)),
                Some((_, best_ts)) if line.timestamp_ms >= best_ts => {
                    best = Some((idx, line.timestamp_ms));
                }
                _ => {}
            }
        }
    }
    best.map(|(idx, _)| idx)
}

pub struct MusicTransportEngine {
    queue: Vec<MusicTrackEntry>,
    play_order: Vec<usize>,
    order_cursor: Option<usize>,
    state: MusicPlaybackState,
    position_ms: u64,
    volume: f32,
    repeat_mode: MusicRepeatMode,
    shuffle: bool,
    shuffle_seed: u64,
    os_audio_enabled: bool,
    os_backend: AudioPlaybackEngine,
    last_os_audio_warning: Option<String>,
}

impl Default for MusicTransportEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicTransportEngine {
    pub fn new() -> Self {
        Self::with_os_audio(false)
    }

    pub fn with_os_audio(enabled: bool) -> Self {
        Self {
            queue: Vec::new(),
            play_order: Vec::new(),
            order_cursor: None,
            state: MusicPlaybackState::Stopped,
            position_ms: 0,
            volume: 1.0,
            repeat_mode: MusicRepeatMode::Off,
            shuffle: false,
            shuffle_seed: DEFAULT_SHUFFLE_SEED,
            os_audio_enabled: enabled,
            os_backend: AudioPlaybackEngine::new(),
            last_os_audio_warning: None,
        }
    }

    pub fn set_os_audio_enabled(&mut self, enabled: bool) {
        if self.os_audio_enabled == enabled {
            return;
        }
        self.os_audio_enabled = enabled;
        if !enabled {
            self.os_backend.stop();
        } else if self.state == MusicPlaybackState::Playing {
            self.sync_os_audio_from_current_pos();
        }
    }

    pub fn is_os_audio_enabled(&self) -> bool {
        self.os_audio_enabled
    }

    pub fn last_os_audio_warning(&self) -> Option<&str> {
        self.last_os_audio_warning.as_deref()
    }

    pub fn set_queue(&mut self, tracks: Vec<MusicTrackEntry>) {
        self.os_backend.stop();
        self.queue = tracks;
        self.position_ms = 0;
        self.state = MusicPlaybackState::Stopped;
        if self.queue.is_empty() {
            self.play_order.clear();
            self.order_cursor = None;
        } else {
            self.rebuild_play_order(None);
            self.order_cursor = Some(0);
        }
    }

    pub fn enqueue(&mut self, track: MusicTrackEntry) {
        let new_idx = self.queue.len();
        self.queue.push(track);
        self.play_order.push(new_idx);
        if self.order_cursor.is_none() {
            self.order_cursor = Some(0);
        }
    }

    pub fn clear_queue(&mut self) {
        self.os_backend.stop();
        self.queue.clear();
        self.play_order.clear();
        self.order_cursor = None;
        self.position_ms = 0;
        self.state = MusicPlaybackState::Stopped;
    }

    pub fn queue(&self) -> &[MusicTrackEntry] {
        &self.queue
    }

    pub fn play_order(&self) -> &[usize] {
        &self.play_order
    }

    pub fn current_track_index(&self) -> Option<usize> {
        let cursor = self.order_cursor?;
        self.play_order.get(cursor).copied()
    }

    pub fn current_track(&self) -> Option<&MusicTrackEntry> {
        let idx = self.current_track_index()?;
        self.queue.get(idx)
    }

    pub fn select_track(&mut self, queue_index: usize) -> Result<(), String> {
        if queue_index >= self.queue.len() {
            return Err(format!(
                "queue index {} out of bounds (len={})",
                queue_index,
                self.queue.len()
            ));
        }
        let cursor = self
            .play_order
            .iter()
            .position(|&idx| idx == queue_index)
            .ok_or_else(|| format!("queue index {queue_index} missing from play_order"))?;
        self.os_backend.stop();
        self.order_cursor = Some(cursor);
        self.position_ms = 0;
        self.state = MusicPlaybackState::Stopped;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), String> {
        if self.queue.is_empty() {
            return Err("Music queue is empty".to_string());
        }
        if self.order_cursor.is_none() {
            self.order_cursor = Some(0);
        }
        if self.state == MusicPlaybackState::Ended {
            self.position_ms = 0;
        }
        self.state = MusicPlaybackState::Playing;
        self.last_os_audio_warning = None;
        self.sync_os_audio_from_current_pos();
        Ok(())
    }

    pub fn play_index(&mut self, queue_index: usize) -> Result<(), String> {
        self.select_track(queue_index)?;
        self.play()
    }

    pub fn pause(&mut self) {
        if self.state == MusicPlaybackState::Playing {
            self.state = MusicPlaybackState::Paused;
            if self.os_audio_enabled {
                if let Err(e) = self.os_backend.pause() {
                    self.last_os_audio_warning = Some(e.to_string());
                }
            }
        }
    }

    pub fn toggle_play_pause(&mut self) -> Result<MusicPlaybackState, String> {
        if self.state == MusicPlaybackState::Playing {
            self.pause();
            Ok(self.state)
        } else {
            self.play()?;
            Ok(self.state)
        }
    }

    pub fn stop(&mut self) {
        self.state = MusicPlaybackState::Stopped;
        self.position_ms = 0;
        self.os_backend.stop();
    }

    pub fn next(&mut self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }
        let was_playing = self.state == MusicPlaybackState::Playing;
        let cur = self.order_cursor.unwrap_or(0);
        let next_cursor = if cur + 1 < self.play_order.len() {
            Some(cur + 1)
        } else if self.repeat_mode == MusicRepeatMode::All {
            Some(0)
        } else {
            None
        };

        if let Some(nc) = next_cursor {
            self.order_cursor = Some(nc);
            self.position_ms = 0;
            if was_playing {
                self.sync_os_audio_from_current_pos();
            } else {
                self.state = MusicPlaybackState::Stopped;
                self.os_backend.stop();
            }
            true
        } else {
            false
        }
    }

    pub fn prev(&mut self) -> bool {
        if self.play_order.is_empty() {
            return false;
        }
        let was_playing = self.state == MusicPlaybackState::Playing;
        let cur = self.order_cursor.unwrap_or(0);
        let prev_cursor = if cur > 0 {
            Some(cur - 1)
        } else if self.repeat_mode == MusicRepeatMode::All {
            Some(self.play_order.len() - 1)
        } else {
            None
        };

        if let Some(pc) = prev_cursor {
            self.order_cursor = Some(pc);
            self.position_ms = 0;
            if was_playing {
                self.sync_os_audio_from_current_pos();
            } else {
                self.state = MusicPlaybackState::Stopped;
                self.os_backend.stop();
            }
            true
        } else {
            false
        }
    }

    pub fn seek_ms(&mut self, pos_ms: u64) -> Result<u64, String> {
        let dur = self
            .current_track()
            .ok_or_else(|| "No active track to seek".to_string())?
            .duration_ms;
        let clamped = pos_ms.min(dur);
        self.position_ms = clamped;
        if self.state == MusicPlaybackState::Ended && clamped < dur {
            self.state = MusicPlaybackState::Paused;
        }
        if self.state == MusicPlaybackState::Playing && self.os_audio_enabled {
            self.sync_os_audio_from_current_pos();
        }
        Ok(clamped)
    }

    pub fn sliced_wav_from_position(&self, pos_ms: u64) -> Result<Vec<u8>, String> {
        let track = self
            .current_track()
            .ok_or_else(|| "No active track in queue".to_string())?;
        if !track.pcm_samples.is_empty() {
            slice_pcm_to_wav_from_ms(
                &track.pcm_samples,
                track.sample_rate,
                track.channels,
                pos_ms,
            )
        } else if !track.wav_bytes.is_empty() {
            slice_wav_from_offset_ms(&track.wav_bytes, pos_ms)
        } else {
            Err("Current track does not carry PCM or WAV audio bytes".to_string())
        }
    }

    pub fn tick_ms(&mut self, delta_ms: u64) -> MusicPlaybackState {
        if self.state != MusicPlaybackState::Playing || delta_ms == 0 {
            return self.state;
        }
        if self.play_order.is_empty() {
            self.state = MusicPlaybackState::Stopped;
            return self.state;
        }

        let mut remaining: u128 = u128::from(delta_ms);
        let mut needs_os_resync = false;
        let cycle_dur: u128 = if self.repeat_mode == MusicRepeatMode::All {
            let total = self.play_order.iter().fold(0u128, |acc, &idx| {
                acc.saturating_add(u128::from(self.queue[idx].duration_ms))
            });
            if total == 0 {
                self.position_ms = 0;
                return self.state;
            }
            if remaining >= total {
                remaining %= total;
                needs_os_resync = true;
            }
            total
        } else {
            0
        };

        loop {
            let cur = self.order_cursor.unwrap_or(0);
            let track_idx = self.play_order[cur];
            let dur = self.queue[track_idx].duration_ms;

            if dur == 0 {
                match self.repeat_mode {
                    MusicRepeatMode::One => {
                        self.position_ms = 0;
                        break;
                    }
                    MusicRepeatMode::Off => {
                        if cur + 1 < self.play_order.len() {
                            self.order_cursor = Some(cur + 1);
                            self.position_ms = 0;
                            needs_os_resync = true;
                            continue;
                        } else {
                            self.position_ms = 0;
                            self.state = MusicPlaybackState::Ended;
                            self.os_backend.stop();
                            needs_os_resync = false;
                            break;
                        }
                    }
                    MusicRepeatMode::All => {
                        self.order_cursor = Some((cur + 1) % self.play_order.len());
                        self.position_ms = 0;
                        needs_os_resync = true;
                        continue;
                    }
                }
            }

            let dur128 = u128::from(dur);
            let target = u128::from(self.position_ms) + remaining;
            if target < dur128 {
                self.position_ms = target as u64;
                break;
            }

            let overflow = target - dur128;
            match self.repeat_mode {
                MusicRepeatMode::One => {
                    self.position_ms = (overflow % dur128) as u64;
                    needs_os_resync = true;
                    break;
                }
                MusicRepeatMode::Off => {
                    if cur + 1 < self.play_order.len() {
                        self.order_cursor = Some(cur + 1);
                        self.position_ms = 0;
                        remaining = overflow;
                        needs_os_resync = true;
                    } else {
                        self.position_ms = dur;
                        self.state = MusicPlaybackState::Ended;
                        self.os_backend.stop();
                        needs_os_resync = false;
                        break;
                    }
                }
                MusicRepeatMode::All => {
                    self.order_cursor = Some((cur + 1) % self.play_order.len());
                    self.position_ms = 0;
                    remaining = if cycle_dur > 0 {
                        overflow % cycle_dur
                    } else {
                        0
                    };
                    needs_os_resync = true;
                }
            }
        }

        if needs_os_resync && self.state == MusicPlaybackState::Playing {
            self.sync_os_audio_from_current_pos();
        }
        self.state
    }

    pub fn set_volume(&mut self, volume: f32) {
        let clamped = if volume.is_nan() {
            0.0
        } else {
            volume.clamp(0.0, 1.0)
        };
        self.volume = clamped;
        self.os_backend.set_volume(clamped);
        if self.state == MusicPlaybackState::Playing && self.os_audio_enabled {
            self.sync_os_audio_from_current_pos();
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_repeat_mode(&mut self, mode: MusicRepeatMode) {
        self.repeat_mode = mode;
    }

    pub fn repeat_mode(&self) -> MusicRepeatMode {
        self.repeat_mode
    }

    pub fn set_shuffle(&mut self, enabled: bool) {
        if self.shuffle == enabled {
            return;
        }
        let active = self.current_track_index();
        self.shuffle = enabled;
        self.rebuild_play_order(active);
    }

    pub fn is_shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn set_shuffle_seed(&mut self, seed: u64) {
        self.shuffle_seed = seed;
        if self.shuffle {
            let active = self.current_track_index();
            self.rebuild_play_order(active);
        }
    }

    pub fn current_lyric_index(&self, lyrics: &[SynthLyricLine]) -> Option<usize> {
        find_lyric_index_at(lyrics, self.position_ms)
    }

    pub fn active_lyric_index(&self) -> Option<usize> {
        let track = self.current_track()?;
        find_lyric_index_at(&track.lyrics, self.position_ms)
    }

    pub fn state(&self) -> MusicPlaybackState {
        self.state
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }

    pub fn duration_ms(&self) -> u64 {
        self.current_track().map(|t| t.duration_ms).unwrap_or(0)
    }

    pub fn snapshot(&self) -> MusicTransportSnapshot {
        let duration_ms = self.duration_ms();
        let progress_ratio = if duration_ms == 0 {
            0.0
        } else {
            (self.position_ms as f64 / duration_ms as f64).clamp(0.0, 1.0) as f32
        };
        MusicTransportSnapshot {
            state: self.state,
            current_index: self.current_track_index(),
            current_track_id: self.current_track().map(|t| t.track_id.clone()),
            position_ms: self.position_ms,
            duration_ms,
            progress_ratio,
            volume: self.volume,
            repeat_mode: self.repeat_mode,
            shuffle: self.shuffle,
            queue_len: self.queue.len(),
            active_lyric_index: self.active_lyric_index(),
        }
    }

    fn rebuild_play_order(&mut self, active_queue_index: Option<usize>) {
        let len = self.queue.len();
        self.play_order = (0..len).collect();
        if len == 0 {
            self.order_cursor = None;
            return;
        }
        if !self.shuffle {
            self.order_cursor = active_queue_index.filter(|&idx| idx < len).or(Some(0));
            return;
        }

        let start = if let Some(active) = active_queue_index.filter(|&idx| idx < len) {
            self.play_order.swap(0, active);
            self.order_cursor = Some(0);
            1
        } else {
            self.order_cursor = Some(0);
            0
        };

        let mut rng_state = splitmix64(self.shuffle_seed ^ (len as u64));
        if len > start + 1 {
            for i in ((start + 1)..len).rev() {
                rng_state = xorshift64(rng_state);
                let span = (i - start + 1) as u64;
                let j = start + (rng_state % span) as usize;
                self.play_order.swap(i, j);
            }
        }
    }

    fn sync_os_audio_from_current_pos(&mut self) {
        if !self.os_audio_enabled {
            return;
        }
        let Some(track_id) = self.current_track().map(|t| t.track_id.clone()) else {
            return;
        };
        match self.sliced_wav_from_position(self.position_ms) {
            Ok(wav) => {
                self.os_backend.set_volume(self.volume);
                if let Err(err) = self.os_backend.play_wav_bytes(track_id, &wav) {
                    self.last_os_audio_warning = Some(err.to_string());
                }
            }
            Err(err) => {
                self.last_os_audio_warning = Some(err);
            }
        }
    }
}

impl Drop for MusicTransportEngine {
    fn drop(&mut self) {
        self.os_backend.stop();
    }
}

fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let mixed = z ^ (z >> 31);
    if mixed == 0 {
        0xD1B5_4A32_D192_ED03
    } else {
        mixed
    }
}

fn xorshift64(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}
