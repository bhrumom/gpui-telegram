//! Pure-Rust polyphonic music synthesizer producing canonical 16-bit PCM RIFF/WAV
//! audio, 100-bar 5-bit Telegram waveforms, and frame-accurate seekable WAV slices.

use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_4, TAU};

pub const DEFAULT_SYNTH_SAMPLE_RATE: u32 = 44_100;
pub const MIN_SYNTH_SAMPLE_RATE: u32 = 8_000;
pub const MAX_SYNTH_SAMPLE_RATE: u32 = 96_000;
pub const MIN_SYNTH_BPM: u16 = 30;
pub const MAX_SYNTH_BPM: u16 = 300;
pub const MAX_SYNTH_DURATION_SECS: f32 = 300.0;

fn default_sample_rate() -> u32 {
    DEFAULT_SYNTH_SAMPLE_RATE
}

fn default_channels() -> u16 {
    1
}

fn default_chord_velocity() -> u8 {
    68
}

/// Timbre presets supported by the pure-Rust synthesizer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SynthWaveformKind {
    #[default]
    Sine,
    Triangle,
    Saw,
    Square,
    Piano,
    Pad,
    Bass,
    Percussion,
}

impl SynthWaveformKind {
    pub const ALL: [Self; 8] = [
        Self::Sine,
        Self::Triangle,
        Self::Saw,
        Self::Square,
        Self::Piano,
        Self::Pad,
        Self::Bass,
        Self::Percussion,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sine => "sine",
            Self::Triangle => "triangle",
            Self::Saw => "saw",
            Self::Square => "square",
            Self::Piano => "piano",
            Self::Pad => "pad",
            Self::Bass => "bass",
            Self::Percussion => "percussion",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "sine" => Some(Self::Sine),
            "triangle" => Some(Self::Triangle),
            "saw" => Some(Self::Saw),
            "square" => Some(Self::Square),
            "piano" => Some(Self::Piano),
            "pad" => Some(Self::Pad),
            "bass" => Some(Self::Bass),
            "percussion" => Some(Self::Percussion),
            _ => None,
        }
    }
}

/// Click-free ADSR amplitude envelope parameters in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SynthAdsr {
    pub attack_secs: f32,
    pub decay_secs: f32,
    pub sustain_level: f32,
    pub release_secs: f32,
}

impl SynthAdsr {
    pub fn for_waveform(kind: SynthWaveformKind) -> Self {
        match kind {
            SynthWaveformKind::Sine => Self {
                attack_secs: 0.010,
                decay_secs: 0.080,
                sustain_level: 0.78,
                release_secs: 0.080,
            },
            SynthWaveformKind::Triangle => Self {
                attack_secs: 0.012,
                decay_secs: 0.080,
                sustain_level: 0.75,
                release_secs: 0.090,
            },
            SynthWaveformKind::Saw => Self {
                attack_secs: 0.008,
                decay_secs: 0.100,
                sustain_level: 0.65,
                release_secs: 0.080,
            },
            SynthWaveformKind::Square => Self {
                attack_secs: 0.006,
                decay_secs: 0.080,
                sustain_level: 0.60,
                release_secs: 0.070,
            },
            SynthWaveformKind::Piano => Self {
                attack_secs: 0.004,
                decay_secs: 0.220,
                sustain_level: 0.35,
                release_secs: 0.180,
            },
            SynthWaveformKind::Pad => Self {
                attack_secs: 0.120,
                decay_secs: 0.250,
                sustain_level: 0.80,
                release_secs: 0.320,
            },
            SynthWaveformKind::Bass => Self {
                attack_secs: 0.006,
                decay_secs: 0.120,
                sustain_level: 0.68,
                release_secs: 0.090,
            },
            SynthWaveformKind::Percussion => Self {
                attack_secs: 0.001,
                decay_secs: 0.095,
                sustain_level: 0.0,
                release_secs: 0.060,
            },
        }
    }

    /// Evaluates continuous, click-free ADSR amplitude at `elapsed_secs` for a note held `hold_secs`.
    pub fn amplitude_at(&self, elapsed_secs: f32, hold_secs: f32) -> f32 {
        if !self.attack_secs.is_finite()
            || !self.decay_secs.is_finite()
            || !self.sustain_level.is_finite()
            || !self.release_secs.is_finite()
            || !elapsed_secs.is_finite()
            || !hold_secs.is_finite()
            || elapsed_secs < 0.0
        {
            return 0.0;
        }
        let a = self.attack_secs.max(0.0);
        let d = self.decay_secs.max(0.0);
        let s = self.sustain_level.clamp(0.0, 1.0);
        let r = self.release_secs.max(0.0);
        let hold = hold_secs.max(0.0);

        let note_on_amp = |t: f32| -> f32 {
            if a > 0.0 && t < a {
                (t / a).clamp(0.0, 1.0)
            } else if d > 0.0 && t < a + d {
                let frac = ((t - a) / d).clamp(0.0, 1.0);
                (1.0 - (1.0 - s) * frac).clamp(0.0, 1.0)
            } else {
                s
            }
        };

        let amp = if elapsed_secs <= hold {
            note_on_amp(elapsed_secs)
        } else {
            let rel_t = elapsed_secs - hold;
            if r <= 0.0 || rel_t >= r {
                0.0
            } else {
                let start_amp = note_on_amp(hold);
                (start_amp * (1.0 - rel_t / r)).clamp(0.0, 1.0)
            }
        };
        if amp.is_finite() {
            amp.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthInstrument {
    pub channel: u8,
    pub name: String,
    pub waveform: SynthWaveformKind,
    pub gain: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adsr: Option<SynthAdsr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthNote {
    pub midi_note: u8,
    pub start_beat: f32,
    pub duration_beats: f32,
    pub velocity: u8,
    pub channel: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthChord {
    pub symbol: String,
    pub start_beat: f32,
    pub duration_beats: f32,
    #[serde(default = "default_chord_velocity")]
    pub velocity: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynthLyricLine {
    pub timestamp_ms: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SynthTrackInput {
    pub track_id: String,
    pub title: String,
    pub bpm: u16,
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_channels")]
    pub channels: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_secs_hint: Option<f32>,
    #[serde(default)]
    pub instruments: Vec<SynthInstrument>,
    #[serde(default)]
    pub melody_notes: Vec<SynthNote>,
    #[serde(default)]
    pub chord_progression: Vec<SynthChord>,
    #[serde(default)]
    pub lyrics: Vec<SynthLyricLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizedTrackAudio {
    pub wav_bytes: Vec<u8>,
    pub pcm_samples: Vec<i16>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
    pub waveform_bars: Vec<u8>,
    pub waveform_5bit: Vec<u8>,
    pub peak_rms: u16,
}

/// Converts MIDI note number (`0..=127`, A4 = 69 = 440.0 Hz) to frequency in Hz.
pub fn midi_note_to_hz(midi_note: u8) -> f64 {
    440.0 * 2.0_f64.powf((f64::from(midi_note) - 69.0) / 12.0)
}

fn parse_pitch_class(input: &str) -> Option<(i32, usize)> {
    let mut chars = input.chars();
    let root_ch = chars.next()?;
    let base: i32 = match root_ch.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let mut consumed = root_ch.len_utf8();
    let mut accidental: i32 = 0;
    if let Some(next_ch) = chars.next() {
        match next_ch {
            '#' | '♯' => {
                accidental = 1;
                consumed += next_ch.len_utf8();
            }
            'b' | '♭' => {
                accidental = -1;
                consumed += next_ch.len_utf8();
            }
            _ => {}
        }
    }
    Some(((base + accidental).rem_euclid(12), consumed))
}

/// Parses chord symbols (e.g. `"C"`, `"Am7"`, `"Fmaj7"`, `"G7"`, `"F#m"`, `"Bb/D"`)
/// into sorted MIDI note numbers around `base_octave` (e.g. 3 for C3 = 48).
pub fn parse_chord_symbol_to_midi(symbol: &str, base_octave: i8) -> Vec<u8> {
    let trimmed = symbol.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let (chord_part, slash_bass_part) = match trimmed.split_once('/') {
        Some((c, b)) => (c.trim(), Some(b.trim())),
        None => (trimmed, None),
    };

    let Some((root_pc, consumed)) = parse_pitch_class(chord_part) else {
        return Vec::new();
    };
    let quality = chord_part[consumed..].trim();
    let quality_lower = quality.to_ascii_lowercase();

    let intervals: &[i32] = if quality == "M7" || quality == "Δ7" || quality_lower == "maj7" {
        &[0, 4, 7, 11]
    } else if quality == "M9" || quality == "Δ9" || quality_lower == "maj9" {
        &[0, 4, 7, 11, 14]
    } else if quality == "M6" || quality_lower == "maj6" || quality_lower == "6" {
        &[0, 4, 7, 9]
    } else if quality.is_empty()
        || quality == "M"
        || quality == "Δ"
        || quality_lower == "maj"
        || quality_lower == "major"
    {
        &[0, 4, 7]
    } else if quality == "m7b5"
        || quality == "m7♭5"
        || quality_lower == "min7b5"
        || quality_lower == "min7♭5"
        || quality_lower == "ø"
        || quality_lower == "ø7"
    {
        &[0, 3, 6, 10]
    } else if quality_lower == "dim7" || quality_lower == "°7" {
        &[0, 3, 6, 9]
    } else if quality_lower == "dim" || quality_lower == "°" {
        &[0, 3, 6]
    } else if quality_lower == "aug" || quality_lower == "+" {
        &[0, 4, 8]
    } else if quality == "m7" || quality_lower == "min7" || quality == "-7" {
        &[0, 3, 7, 10]
    } else if quality == "m9" || quality_lower == "min9" || quality == "-9" {
        &[0, 3, 7, 10, 14]
    } else if quality_lower == "sus2" {
        &[0, 2, 7]
    } else if quality_lower == "sus4" || quality_lower == "sus" {
        &[0, 5, 7]
    } else if quality == "m6" || quality_lower == "min6" {
        &[0, 3, 7, 9]
    } else if quality_lower == "7" || quality_lower == "dom7" {
        &[0, 4, 7, 10]
    } else if quality_lower == "9" {
        &[0, 4, 7, 10, 14]
    } else if quality_lower == "add9" {
        &[0, 4, 7, 14]
    } else if quality_lower == "5" {
        &[0, 7, 12]
    } else if quality == "m" || quality == "-" || quality_lower == "min" || quality_lower == "minor"
    {
        &[0, 3, 7]
    } else {
        return Vec::new();
    };

    let root_midi = ((i32::from(base_octave) + 1) * 12 + root_pc).clamp(12, 96);
    let mut notes: Vec<u8> = intervals
        .iter()
        .map(|&iv| (root_midi + iv).clamp(0, 127) as u8)
        .collect();

    if let Some(bass_str) = slash_bass_part {
        if bass_str.is_empty() {
            return Vec::new();
        }
        let Some((bass_pc, bass_consumed)) = parse_pitch_class(bass_str) else {
            return Vec::new();
        };
        if bass_consumed != bass_str.len() {
            return Vec::new();
        }
        let mut bass_midi = (i32::from(base_octave) + 1) * 12 + bass_pc;
        while bass_midi >= root_midi {
            bass_midi -= 12;
        }
        if bass_midi < 24 {
            bass_midi += 12;
        }
        notes.push(bass_midi.clamp(0, 127) as u8);
    }

    notes.sort_unstable();
    notes.dedup();
    notes
}

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn from_seed(seed: u64) -> Self {
        // SplitMix64 mixing so small sequential seeds yield decorrelated streams
        let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        let mixed = z ^ (z >> 31);
        Self {
            state: if mixed == 0 { 0xD1B5_4A32_D192_ED03 } else { mixed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_bipolar(&mut self) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64);
        unit * 2.0 - 1.0
    }
}

fn sample_oscillator(
    kind: SynthWaveformKind,
    freq: f64,
    t: f64,
    midi_note: u8,
    rng: &mut XorShift64,
    prev_noise: &mut f64,
) -> f64 {
    match kind {
        SynthWaveformKind::Sine => (TAU * freq * t).sin(),
        SynthWaveformKind::Triangle => {
            let p = (freq * t + 0.25).fract();
            1.0 - 4.0 * (p - 0.5).abs()
        }
        SynthWaveformKind::Saw => {
            let p = (freq * t).fract();
            2.0 * p - 1.0
        }
        SynthWaveformKind::Square => {
            if (TAU * freq * t).sin() >= 0.0 {
                0.82
            } else {
                -0.82
            }
        }
        SynthWaveformKind::Piano => {
            let w = TAU * freq;
            let h1 = (w * t).sin() * (-1.5 * t).exp();
            let h2 = (2.0 * w * t).sin() * (-3.1 * t).exp();
            let h3 = (3.0 * w * t).sin() * (-5.2 * t).exp();
            let h4 = (4.0 * w * t).sin() * (-7.8 * t).exp();
            0.56 * h1 + 0.25 * h2 + 0.13 * h3 + 0.06 * h4
        }
        SynthWaveformKind::Pad => {
            let v1 = (TAU * (freq * 0.996) * t).sin();
            let v2 = (TAU * freq * t).sin() + 0.18 * (TAU * 2.0 * freq * t).sin();
            let v3 = (TAU * (freq * 1.004) * t).sin();
            (v1 + v2 + v3) / 3.18
        }
        SynthWaveformKind::Bass => {
            let w = TAU * freq;
            0.56 * (w * t).sin() + 0.30 * (0.5 * w * t).sin() + 0.14 * (2.0 * w * t).sin()
        }
        SynthWaveformKind::Percussion => {
            let noise = rng.next_bipolar();
            let hp_noise = noise - *prev_noise;
            *prev_noise = noise;
            if midi_note <= 40 {
                // Kick drum: rapid downward pitch sweep + subtle click transient
                let sweep_phase = TAU * (48.0 * t + (115.0 / 28.0) * (1.0 - (-28.0 * t).exp()));
                let body = sweep_phase.sin() * (-12.0 * t).exp();
                let click = noise * (-120.0 * t).exp() * 0.25;
                (body + click).clamp(-1.0, 1.0)
            } else if midi_note <= 50 {
                // Snare drum: tonal body + filtered noise burst
                let body = (TAU * 190.0 * t).sin() * (-18.0 * t).exp() * 0.45;
                let wires = (0.65 * noise + 0.35 * hp_noise) * (-14.0 * t).exp() * 0.70;
                (body + wires).clamp(-1.0, 1.0)
            } else {
                // Hi-hat / cymbal: high-passed noise + metallic partials
                let metallic =
                    ((TAU * 3810.0 * t).sin() + (TAU * 5730.0 * t).sin()) * 0.15;
                ((0.65 * hp_noise + metallic) * (-32.0 * t).exp()).clamp(-1.0, 1.0)
            }
        }
    }
}

struct RenderVoice<'a> {
    midi_note: u8,
    start_secs: f64,
    hold_secs: f64,
    velocity: f64,
    waveform: SynthWaveformKind,
    adsr: SynthAdsr,
    gain: f64,
    pan: f64,
    seed: u64,
    sample_rate: f64,
    channels: u16,
    mix_l: &'a mut [f64],
    mix_r: &'a mut [f64],
}

fn render_voice(v: RenderVoice<'_>) {
    let total_secs = v.hold_secs + f64::from(v.adsr.release_secs.max(0.0));
    let start_frame = (v.start_secs * v.sample_rate).round().max(0.0) as usize;
    let end_frame = (((v.start_secs + total_secs) * v.sample_rate).ceil().max(0.0) as usize)
        .min(v.mix_l.len());
    if start_frame >= end_frame {
        return;
    }

    let freq = midi_note_to_hz(v.midi_note);
    let base_amp = v.velocity * v.gain.clamp(0.0, 2.0);
    let (gain_l, gain_r) = if v.channels == 2 {
        let angle = (v.pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
        (angle.cos(), angle.sin())
    } else {
        (1.0, 0.0)
    };

    let mut rng = XorShift64::from_seed(v.seed);
    let mut prev_noise = 0.0_f64;

    for frame in start_frame..end_frame {
        let elapsed = (frame - start_frame) as f64 / v.sample_rate;
        let env = f64::from(v.adsr.amplitude_at(elapsed as f32, v.hold_secs as f32));
        if !env.is_finite() || env <= 0.0 {
            continue;
        }
        let raw = sample_oscillator(v.waveform, freq, elapsed, v.midi_note, &mut rng, &mut prev_noise);
        let sample = raw * env * base_amp;
        if v.channels == 2 {
            v.mix_l[frame] += sample * gain_l;
            v.mix_r[frame] += sample * gain_r;
        } else {
            v.mix_l[frame] += sample;
        }
    }
}

fn validate_synth_input(track: &SynthTrackInput) -> Result<(), String> {
    if !(MIN_SYNTH_BPM..=MAX_SYNTH_BPM).contains(&track.bpm) {
        return Err(format!(
            "BPM {} out of valid range {}..={}",
            track.bpm, MIN_SYNTH_BPM, MAX_SYNTH_BPM
        ));
    }
    if !(MIN_SYNTH_SAMPLE_RATE..=MAX_SYNTH_SAMPLE_RATE).contains(&track.sample_rate) {
        return Err(format!(
            "sample_rate {} out of valid range {}..={}",
            track.sample_rate, MIN_SYNTH_SAMPLE_RATE, MAX_SYNTH_SAMPLE_RATE
        ));
    }
    if track.channels != 1 && track.channels != 2 {
        return Err(format!(
            "channels must be 1 (mono) or 2 (stereo), got {}",
            track.channels
        ));
    }
    if track.melody_notes.is_empty() && track.chord_progression.is_empty() {
        return Err("track must contain at least one melody note or chord".to_string());
    }
    if let Some(hint) = track.duration_secs_hint {
        if !hint.is_finite() || hint <= 0.0 {
            return Err("duration_secs_hint must be finite and > 0.0".to_string());
        }
    }
    for (i, inst) in track.instruments.iter().enumerate() {
        if !inst.gain.is_finite() || inst.gain < 0.0 {
            return Err(format!("instruments[{i}].gain must be finite and >= 0.0"));
        }
        if !inst.pan.is_finite() || inst.pan < -1.0 || inst.pan > 1.0 {
            return Err(format!("instruments[{i}].pan must be finite and within -1.0..=1.0"));
        }
        if let Some(adsr) = inst.adsr {
            if !adsr.attack_secs.is_finite()
                || adsr.attack_secs < 0.0
                || adsr.attack_secs > MAX_SYNTH_DURATION_SECS
            {
                return Err(format!(
                    "instruments[{i}].adsr.attack_secs must be finite and within 0.0..={MAX_SYNTH_DURATION_SECS}"
                ));
            }
            if !adsr.decay_secs.is_finite()
                || adsr.decay_secs < 0.0
                || adsr.decay_secs > MAX_SYNTH_DURATION_SECS
            {
                return Err(format!(
                    "instruments[{i}].adsr.decay_secs must be finite and within 0.0..={MAX_SYNTH_DURATION_SECS}"
                ));
            }
            if !adsr.release_secs.is_finite()
                || adsr.release_secs < 0.0
                || adsr.release_secs > MAX_SYNTH_DURATION_SECS
            {
                return Err(format!(
                    "instruments[{i}].adsr.release_secs must be finite and within 0.0..={MAX_SYNTH_DURATION_SECS}"
                ));
            }
            if !adsr.sustain_level.is_finite() || !(0.0..=1.0).contains(&adsr.sustain_level) {
                return Err(format!(
                    "instruments[{i}].adsr.sustain_level must be finite and within 0.0..=1.0"
                ));
            }
        }
    }
    for (i, note) in track.melody_notes.iter().enumerate() {
        if note.midi_note > 127 {
            return Err(format!("melody_notes[{i}].midi_note {} exceeds 127", note.midi_note));
        }
        if !note.start_beat.is_finite() || note.start_beat < 0.0 {
            return Err(format!("melody_notes[{i}].start_beat must be finite and >= 0.0"));
        }
        if !note.duration_beats.is_finite() || note.duration_beats <= 0.0 {
            return Err(format!("melody_notes[{i}].duration_beats must be finite and > 0.0"));
        }
    }
    for (i, chord) in track.chord_progression.iter().enumerate() {
        if !chord.start_beat.is_finite() || chord.start_beat < 0.0 {
            return Err(format!("chord_progression[{i}].start_beat must be finite and >= 0.0"));
        }
        if !chord.duration_beats.is_finite() || chord.duration_beats <= 0.0 {
            return Err(format!(
                "chord_progression[{i}].duration_beats must be finite and > 0.0"
            ));
        }
    }
    Ok(())
}

/// Synthesizes a polyphonic `SynthTrackInput` into canonical 16-bit PCM, RIFF/WAV bytes,
/// 100-bar waveform peaks, 63-byte packed Telegram 5-bit waveform, and RMS loudness.
pub fn synthesize_music_track_wav(track: &SynthTrackInput) -> Result<SynthesizedTrackAudio, String> {
    validate_synth_input(track)?;

    let secs_per_beat = 60.0 / f64::from(track.bpm);
    let sample_rate = track.sample_rate;
    let sr_f64 = f64::from(sample_rate);
    let channels = track.channels;

    let default_inst = SynthInstrument {
        channel: 0,
        name: "Default Piano".to_string(),
        waveform: SynthWaveformKind::Piano,
        gain: 0.78,
        pan: 0.0,
        adsr: None,
    };
    let default_chord_inst = SynthInstrument {
        channel: 255,
        name: "Warm Pad".to_string(),
        waveform: SynthWaveformKind::Pad,
        gain: 0.42,
        pan: 0.0,
        adsr: None,
    };

    let find_instrument = |ch: u8| -> &SynthInstrument {
        track
            .instruments
            .iter()
            .find(|inst| inst.channel == ch)
            .unwrap_or(&default_inst)
    };

    let mut max_end_secs = track.duration_secs_hint.map(f64::from).unwrap_or(0.0);
    for note in &track.melody_notes {
        let inst = find_instrument(note.channel);
        let adsr = inst.adsr.unwrap_or_else(|| SynthAdsr::for_waveform(inst.waveform));
        let end = (f64::from(note.start_beat) + f64::from(note.duration_beats)) * secs_per_beat
            + f64::from(adsr.release_secs.max(0.0));
        if end > max_end_secs {
            max_end_secs = end;
        }
    }
    for chord in &track.chord_progression {
        let inst = chord
            .channel
            .and_then(|ch| track.instruments.iter().find(|i| i.channel == ch))
            .unwrap_or(&default_chord_inst);
        let adsr = inst.adsr.unwrap_or_else(|| SynthAdsr::for_waveform(inst.waveform));
        let end = (f64::from(chord.start_beat) + f64::from(chord.duration_beats)) * secs_per_beat
            + f64::from(adsr.release_secs.max(0.0));
        if end > max_end_secs {
            max_end_secs = end;
        }
    }

    let capped_secs = max_end_secs.clamp(0.05, f64::from(MAX_SYNTH_DURATION_SECS));
    let total_frames = ((capped_secs * sr_f64).ceil() as usize).max(100);
    let mut mix_l = vec![0.0_f64; total_frames];
    let mut mix_r = if channels == 2 {
        vec![0.0_f64; total_frames]
    } else {
        Vec::new()
    };

    let mut voices_rendered = 0usize;
    for (idx, note) in track.melody_notes.iter().enumerate() {
        let inst = find_instrument(note.channel);
        let adsr = inst.adsr.unwrap_or_else(|| SynthAdsr::for_waveform(inst.waveform));
        let velocity = (f64::from(note.velocity.min(127)) / 127.0).max(0.05);
        let seed = ((idx as u64).wrapping_add(1) << 16)
            ^ (u64::from(note.midi_note) << 8)
            ^ u64::from(note.channel);
        render_voice(RenderVoice {
            midi_note: note.midi_note,
            start_secs: f64::from(note.start_beat) * secs_per_beat,
            hold_secs: f64::from(note.duration_beats) * secs_per_beat,
            velocity,
            waveform: inst.waveform,
            adsr,
            gain: f64::from(inst.gain),
            pan: f64::from(inst.pan),
            seed,
            sample_rate: sr_f64,
            channels,
            mix_l: &mut mix_l,
            mix_r: &mut mix_r,
        });
        voices_rendered += 1;
    }

    for (cidx, chord) in track.chord_progression.iter().enumerate() {
        let midi_notes = parse_chord_symbol_to_midi(&chord.symbol, 3);
        if midi_notes.is_empty() {
            continue;
        }
        let inst = chord
            .channel
            .and_then(|ch| track.instruments.iter().find(|i| i.channel == ch))
            .unwrap_or(&default_chord_inst);
        let adsr = inst.adsr.unwrap_or_else(|| SynthAdsr::for_waveform(inst.waveform));
        let per_voice_gain = f64::from(inst.gain) / (midi_notes.len() as f64).sqrt();
        let velocity = (f64::from(chord.velocity.min(127)) / 127.0).max(0.05);
        for (nidx, midi_note) in midi_notes.into_iter().enumerate() {
            let seed = 0x5A00_0000_u64
                ^ ((cidx as u64) << 16)
                ^ ((nidx as u64) << 8)
                ^ u64::from(midi_note);
            render_voice(RenderVoice {
                midi_note,
                start_secs: f64::from(chord.start_beat) * secs_per_beat,
                hold_secs: f64::from(chord.duration_beats) * secs_per_beat,
                velocity,
                waveform: inst.waveform,
                adsr,
                gain: per_voice_gain,
                pan: f64::from(inst.pan),
                seed,
                sample_rate: sr_f64,
                channels,
                mix_l: &mut mix_l,
                mix_r: &mut mix_r,
            });
            voices_rendered += 1;
        }
    }

    if voices_rendered == 0 {
        return Err("No playable notes or recognized chord symbols in track".to_string());
    }

    // Peak headroom normalization + soft ceiling to avoid digital clipping
    let mut peak = 0.0_f64;
    for &s in &mix_l {
        peak = peak.max(s.abs());
    }
    for &s in &mix_r {
        peak = peak.max(s.abs());
    }
    let norm = if peak > 0.88 { 0.88 / peak } else { 1.0 };

    let to_i16 = |s: f64| -> i16 {
        let scaled = s * norm;
        let limited = if scaled.abs() > 0.92 {
            scaled.tanh() * 0.96
        } else {
            scaled
        };
        (limited.clamp(-1.0, 1.0) * 32767.0).round() as i16
    };

    let mut pcm_samples = Vec::with_capacity(total_frames * usize::from(channels));
    let mut mono_for_waveform = Vec::with_capacity(total_frames);
    if channels == 2 {
        for i in 0..total_frames {
            let l = to_i16(mix_l[i]);
            let r = to_i16(mix_r[i]);
            pcm_samples.push(l);
            pcm_samples.push(r);
            mono_for_waveform.push(((i32::from(l) + i32::from(r)) / 2) as i16);
        }
    } else {
        for i in 0..total_frames {
            let m = to_i16(mix_l[i]);
            pcm_samples.push(m);
            mono_for_waveform.push(m);
        }
    }

    let wav_bytes = super::wav::encode_wav_i16(&pcm_samples, sample_rate, channels)
        .map_err(|e| e.to_string())?;
    let (waveform_bars, waveform_5bit) = super::wav::waveform_from_pcm_i16(&mono_for_waveform);
    let peak_rms = super::wav::compute_pcm_rms_level(&pcm_samples);
    let duration_ms = ((total_frames as u128 * 1000) / u128::from(sample_rate)) as u64;

    Ok(SynthesizedTrackAudio {
        wav_bytes,
        pcm_samples,
        sample_rate,
        channels,
        duration_ms,
        waveform_bars,
        waveform_5bit,
        peak_rms,
    })
}

/// Encodes a frame-aligned RIFF/WAV slice starting at `offset_ms` from interleaved i16 PCM.
pub fn slice_pcm_to_wav_from_ms(
    pcm_samples: &[i16],
    sample_rate: u32,
    channels: u16,
    offset_ms: u64,
) -> Result<Vec<u8>, String> {
    if sample_rate == 0 {
        return Err("sample_rate must be non-zero".to_string());
    }
    if channels != 1 && channels != 2 {
        return Err(format!("channels must be 1 or 2, got {channels}"));
    }
    let ch = usize::from(channels);
    if pcm_samples.len() % ch != 0 {
        return Err("pcm_samples length must be aligned to channel count".to_string());
    }
    let total_frames = pcm_samples.len() / ch;
    let start_frame = ((u128::from(offset_ms) * u128::from(sample_rate)) / 1000)
        .min(total_frames as u128) as usize;
    let sliced = &pcm_samples[start_frame * ch..];
    super::wav::encode_wav_i16(sliced, sample_rate, channels).map_err(|e| e.to_string())
}

/// Decodes a RIFF/WAV buffer and returns a frame-aligned RIFF/WAV starting from `offset_ms`.
pub fn slice_wav_from_offset_ms(wav_bytes: &[u8], offset_ms: u64) -> Result<Vec<u8>, String> {
    let decoded = super::wav::decode_wav_pcm(wav_bytes).map_err(|e| e.to_string())?;
    let ch = usize::from(decoded.channels.max(1));
    let aligned_len = (decoded.samples.len() / ch) * ch;
    slice_pcm_to_wav_from_ms(
        &decoded.samples[..aligned_len],
        decoded.sample_rate,
        decoded.channels,
        offset_ms,
    )
}
