use gpui_telegram_media::{
    decode_wav_pcm, encode_wav_i16, find_lyric_index_at, midi_note_to_hz,
    parse_chord_symbol_to_midi, slice_pcm_to_wav_from_ms, slice_wav_from_offset_ms,
    synthesize_music_track_wav, MusicPlaybackState, MusicRepeatMode, MusicTrackEntry,
    MusicTransportEngine, SynthAdsr, SynthChord, SynthInstrument, SynthLyricLine, SynthNote,
    SynthTrackInput, SynthWaveformKind, DEFAULT_SYNTH_SAMPLE_RATE, TELEGRAM_WAVEFORM_BARS,
    TELEGRAM_WAVEFORM_BYTES,
};
use std::time::Instant;

fn make_simple_track(waveform: SynthWaveformKind, channels: u16, pan: f32) -> SynthTrackInput {
    SynthTrackInput {
        track_id: format!("trk_{}", waveform.as_str()),
        title: format!("Test {}", waveform.as_str()),
        bpm: 120,
        sample_rate: DEFAULT_SYNTH_SAMPLE_RATE,
        channels,
        duration_secs_hint: None,
        instruments: vec![SynthInstrument {
            channel: 0,
            name: waveform.as_str().to_string(),
            waveform,
            gain: 0.8,
            pan,
            adsr: None,
        }],
        melody_notes: vec![
            SynthNote { midi_note: 36, start_beat: 0.0, duration_beats: 0.5, velocity: 110, channel: 0 },
            SynthNote { midi_note: 45, start_beat: 0.5, duration_beats: 0.5, velocity: 105, channel: 0 },
            SynthNote { midi_note: 60, start_beat: 1.0, duration_beats: 0.5, velocity: 100, channel: 0 },
            SynthNote { midi_note: 67, start_beat: 1.5, duration_beats: 0.5, velocity: 100, channel: 0 },
        ],
        chord_progression: vec![],
        lyrics: vec![
            SynthLyricLine { timestamp_ms: 0, text: "Intro".to_string() },
            SynthLyricLine { timestamp_ms: 500, text: "Verse".to_string() },
        ],
    }
}

#[test]
fn test_all_eight_waveforms_deterministic_non_silent_and_valid_wav() {
    assert!((midi_note_to_hz(69) - 440.0).abs() < 1e-9);
    assert!((midi_note_to_hz(60) - 261.625_565_3).abs() < 1e-4);

    for kind in SynthWaveformKind::ALL {
        let input = make_simple_track(kind, 1, 0.0);
        let a = synthesize_music_track_wav(&input)
            .unwrap_or_else(|e| panic!("synth failed for {kind:?}: {e}"));
        let b = synthesize_music_track_wav(&input).expect("deterministic re-synth");

        assert_eq!(a, b, "synthesis must be 100% deterministic for {kind:?}");
        assert!(a.peak_rms > 0, "expected non-silent RMS for {kind:?}");
        assert_eq!(a.sample_rate, DEFAULT_SYNTH_SAMPLE_RATE);
        assert_eq!(a.channels, 1);
        assert_eq!(a.waveform_bars.len(), TELEGRAM_WAVEFORM_BARS);
        assert!(a.waveform_bars.iter().all(|&bar| bar <= 31));
        assert!(a.waveform_bars.iter().any(|&bar| bar > 0));
        assert_eq!(a.waveform_5bit.len(), TELEGRAM_WAVEFORM_BYTES);

        let decoded = decode_wav_pcm(&a.wav_bytes).expect("valid RIFF/WAV output");
        assert_eq!(decoded.sample_rate, a.sample_rate);
        assert_eq!(decoded.channels, a.channels);
        assert_eq!(decoded.samples, a.pcm_samples);
        assert_eq!(decoded.duration_ms as u64, a.duration_ms);
    }
}

#[test]
fn test_stereo_panning_chord_progression_and_validation_rejections() {
    // Hard-left pan vs hard-right pan in stereo
    let left_audio = synthesize_music_track_wav(&make_simple_track(SynthWaveformKind::Sine, 2, -1.0))
        .expect("stereo left");
    assert_eq!(left_audio.pcm_samples.len() % 2, 0);
    let (sum_l, sum_r) = left_audio.pcm_samples.chunks_exact(2).fold((0u64, 0u64), |(l, r), p| {
        (l + p[0].unsigned_abs() as u64, r + p[1].unsigned_abs() as u64)
    });
    assert!(sum_l > 0 && sum_r == 0, "expected hard-left energy only: L={sum_l} R={sum_r}");

    let right_audio = synthesize_music_track_wav(&make_simple_track(SynthWaveformKind::Sine, 2, 1.0))
        .expect("stereo right");
    let (sum_l2, sum_r2) =
        right_audio.pcm_samples.chunks_exact(2).fold((0u64, 0u64), |(l, r), p| {
            (l + p[0].unsigned_abs() as u64, r + p[1].unsigned_abs() as u64)
        });
    assert!(sum_r2 > 0 && sum_l2 == 0, "expected hard-right energy only: L={sum_l2} R={sum_r2}");

    // Chord symbol parsing & chord-only synthesis
    assert_eq!(parse_chord_symbol_to_midi("C", 3), vec![48, 52, 55]);
    assert_eq!(parse_chord_symbol_to_midi("Am7", 3), vec![57, 60, 64, 67]);
    assert_eq!(parse_chord_symbol_to_midi("Fmaj7", 3), vec![53, 57, 60, 64]);
    assert_eq!(parse_chord_symbol_to_midi("G7", 3), vec![55, 59, 62, 65]);
    assert_eq!(parse_chord_symbol_to_midi("F#m", 3), vec![54, 57, 61]);
    assert_eq!(parse_chord_symbol_to_midi("Bb/D", 3), vec![50, 58, 62, 65]);

    let chord_track = SynthTrackInput {
        track_id: "trk_chords".to_string(),
        title: "Progression".to_string(),
        bpm: 100,
        sample_rate: 22_050,
        channels: 2,
        duration_secs_hint: None,
        instruments: vec![],
        melody_notes: vec![],
        chord_progression: vec![
            SynthChord { symbol: "C".into(), start_beat: 0.0, duration_beats: 1.0, velocity: 72, channel: None },
            SynthChord { symbol: "Am7".into(), start_beat: 1.0, duration_beats: 1.0, velocity: 72, channel: None },
            SynthChord { symbol: "Fmaj7".into(), start_beat: 2.0, duration_beats: 1.0, velocity: 72, channel: None },
            SynthChord { symbol: "G7".into(), start_beat: 3.0, duration_beats: 1.0, velocity: 72, channel: None },
        ],
        lyrics: vec![],
    };
    let chord_audio = synthesize_music_track_wav(&chord_track).expect("chord synthesis");
    assert!(chord_audio.peak_rms > 0);
    assert_eq!(chord_audio.waveform_bars.len(), TELEGRAM_WAVEFORM_BARS);

    // Invalid inputs rejected
    let mut bad_bpm_low = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    bad_bpm_low.bpm = 0;
    assert!(synthesize_music_track_wav(&bad_bpm_low).is_err());

    let mut bad_bpm_high = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    bad_bpm_high.bpm = 500;
    assert!(synthesize_music_track_wav(&bad_bpm_high).is_err());

    let mut empty_score = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    empty_score.melody_notes.clear();
    assert!(synthesize_music_track_wav(&empty_score).is_err());

    let mut bad_channels = make_simple_track(SynthWaveformKind::Piano, 3, 0.0);
    bad_channels.channels = 3;
    assert!(synthesize_music_track_wav(&bad_channels).is_err());

    let mut nan_beat = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    nan_beat.melody_notes[0].start_beat = f32::NAN;
    assert!(synthesize_music_track_wav(&nan_beat).is_err());

    let mut neg_beat = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    neg_beat.melody_notes[0].start_beat = -1.0;
    assert!(synthesize_music_track_wav(&neg_beat).is_err());

    let mut bad_midi = make_simple_track(SynthWaveformKind::Piano, 1, 0.0);
    bad_midi.melody_notes[0].midi_note = 128;
    assert!(synthesize_music_track_wav(&bad_midi).is_err());
}

#[test]
fn test_pcm_and_wav_frame_slicing_at_start_mid_and_eof() {
    let audio = synthesize_music_track_wav(&make_simple_track(SynthWaveformKind::Piano, 2, 0.25))
        .expect("synth stereo");
    let ch = usize::from(audio.channels);

    // 0 ms slice matches original PCM identically
    let wav0 = slice_pcm_to_wav_from_ms(&audio.pcm_samples, audio.sample_rate, audio.channels, 0)
        .expect("slice 0ms");
    let dec0 = decode_wav_pcm(&wav0).expect("decode 0ms");
    assert_eq!(dec0.samples, audio.pcm_samples);

    // Mid-track slice (400ms)
    let offset_ms = 400u64;
    let wav_mid = slice_pcm_to_wav_from_ms(
        &audio.pcm_samples,
        audio.sample_rate,
        audio.channels,
        offset_ms,
    )
    .expect("slice mid");
    let dec_mid = decode_wav_pcm(&wav_mid).expect("decode mid");
    let expected_start_frame =
        ((u128::from(offset_ms) * u128::from(audio.sample_rate)) / 1000) as usize;
    assert_eq!(dec_mid.samples, &audio.pcm_samples[expected_start_frame * ch..]);
    let expected_dur = audio.duration_ms.saturating_sub(offset_ms);
    assert!(
        dec_mid.duration_ms.abs_diff(expected_dur as i64) <= 1,
        "got {}ms expected ~{}ms",
        dec_mid.duration_ms,
        expected_dur
    );

    // Slice directly from WAV bytes matches PCM slice
    let wav_mid_from_bytes = slice_wav_from_offset_ms(&audio.wav_bytes, offset_ms)
        .expect("slice from wav bytes");
    assert_eq!(wav_mid_from_bytes, wav_mid);

    // EOF slice returns valid empty-data RIFF/WAV (0ms)
    let wav_eof = slice_pcm_to_wav_from_ms(
        &audio.pcm_samples,
        audio.sample_rate,
        audio.channels,
        audio.duration_ms + 5_000,
    )
    .expect("slice eof");
    let dec_eof = decode_wav_pcm(&wav_eof).expect("decode eof");
    assert!(dec_eof.samples.is_empty());
    assert_eq!(dec_eof.duration_ms, 0);
}

#[test]
fn test_transport_queue_repeat_shuffle_seek_volume_lyrics_and_linux_os_audio() {
    let audio_a = synthesize_music_track_wav(&make_simple_track(SynthWaveformKind::Piano, 1, 0.0))
        .expect("synth a");
    let wav_b = audio_a.wav_bytes.clone();

    let lyrics = vec![
        SynthLyricLine { timestamp_ms: 2000, text: "Chorus".to_string() },
        SynthLyricLine { timestamp_ms: 500, text: "Verse 1".to_string() },
        SynthLyricLine { timestamp_ms: 1200, text: "Pre-Chorus".to_string() },
    ];
    // Unsorted lyric lookup checks: before first, exact, between, after last
    assert_eq!(find_lyric_index_at(&lyrics, 0), None);
    assert_eq!(find_lyric_index_at(&lyrics, 499), None);
    assert_eq!(find_lyric_index_at(&lyrics, 500), Some(1));
    assert_eq!(find_lyric_index_at(&lyrics, 1199), Some(1));
    assert_eq!(find_lyric_index_at(&lyrics, 1200), Some(2));
    assert_eq!(find_lyric_index_at(&lyrics, 2000), Some(0));
    assert_eq!(find_lyric_index_at(&lyrics, 9999), Some(0));

    let t0 = MusicTrackEntry::from_synthesized("t0", "One", "A", "Alb", audio_a, lyrics.clone());
    let t1 = MusicTrackEntry::from_wav_bytes("t1", "Two", "A", "Alb", wav_b, vec![]).expect("from wav");
    let t2 = MusicTrackEntry::clock_only("t2", "Three", "A", "Alb", 1000, vec![]);
    let t3 = MusicTrackEntry::clock_only("t3", "Four", "A", "Alb", 1500, vec![]);

    let mut engine = MusicTransportEngine::new();
    assert!(!engine.is_os_audio_enabled());
    assert!(engine.play().is_err());

    engine.set_queue(vec![t0, t1]);
    engine.enqueue(t2);
    engine.enqueue(t3);
    assert_eq!(engine.queue().len(), 4);

    // Volume clamping + NaN safety
    engine.set_volume(1.5);
    assert_eq!(engine.volume(), 1.0);
    engine.set_volume(-0.4);
    assert_eq!(engine.volume(), 0.0);
    engine.set_volume(f32::NAN);
    assert_eq!(engine.volume(), 0.0);
    engine.set_volume(0.75);
    assert_eq!(engine.volume(), 0.75);

    // Play, seek, lyric sync, sliced WAV from position, pause, toggle, next/prev
    engine.play().expect("play t0");
    assert_eq!(engine.state(), MusicPlaybackState::Playing);
    let sought = engine.seek_ms(600).expect("seek 600ms");
    assert_eq!(sought, 600);
    assert_eq!(engine.active_lyric_index(), Some(1));
    assert_eq!(engine.current_lyric_index(&lyrics), Some(1));
    let sliced = engine.sliced_wav_from_position(600).expect("sliced wav");
    assert!(decode_wav_pcm(&sliced).is_ok());

    assert_eq!(engine.toggle_play_pause().unwrap(), MusicPlaybackState::Paused);
    assert_eq!(engine.toggle_play_pause().unwrap(), MusicPlaybackState::Playing);

    assert!(engine.next());
    assert_eq!(engine.current_track_index(), Some(1));
    assert_eq!(engine.position_ms(), 0);
    assert!(engine.prev());
    assert_eq!(engine.current_track_index(), Some(0));
    assert!(!engine.prev()); // RepeatMode::Off at start

    // Deterministic seeded shuffle preserves currently active track at play_order[0]
    engine.play_index(2).expect("play t2");
    engine.set_shuffle_seed(42);
    engine.set_shuffle(true);
    let order_first = engine.play_order().to_vec();
    assert_eq!(order_first[0], 2);
    assert_eq!(engine.current_track_index(), Some(2));
    engine.set_shuffle(false);
    assert_eq!(engine.play_order(), &[0, 1, 2, 3]);
    assert_eq!(engine.current_track_index(), Some(2));
    engine.set_shuffle(true);
    assert_eq!(engine.play_order(), order_first.as_slice());

    // Repeat modes on tick_ms + large delta + zero-duration safety
    engine.set_shuffle(false);
    engine.set_queue(vec![
        MusicTrackEntry::clock_only("c0", "Zero", "A", "B", 0, vec![]),
        MusicTrackEntry::clock_only("c1", "Short", "A", "B", 1000, vec![]),
        MusicTrackEntry::clock_only("c2", "Long", "A", "B", 2000, vec![]),
    ]);
    engine.set_repeat_mode(MusicRepeatMode::Off);
    engine.play_index(0).unwrap();
    // Zero-duration track immediately advances into c1 (pos 400)
    assert_eq!(engine.tick_ms(400), MusicPlaybackState::Playing);
    assert_eq!(engine.current_track_index(), Some(1));
    assert_eq!(engine.position_ms(), 400);
    // Advance past end of queue -> Ended at c2 duration
    assert_eq!(engine.tick_ms(5000), MusicPlaybackState::Ended);
    assert_eq!(engine.current_track_index(), Some(2));
    assert_eq!(engine.position_ms(), 2000);

    // RepeatMode::One loops modulo track duration in O(1) even for huge delta
    engine.set_repeat_mode(MusicRepeatMode::One);
    engine.play_index(1).unwrap();
    assert_eq!(engine.tick_ms(1_000_000_250), MusicPlaybackState::Playing);
    assert_eq!(engine.current_track_index(), Some(1));
    assert_eq!(engine.position_ms(), 250);

    // RepeatMode::All wraps across full queue even for huge delta
    engine.set_repeat_mode(MusicRepeatMode::All);
    engine.play_index(1).unwrap();
    // Queue non-zero cycle = 1000 + 2000 = 3000ms; 3_000_000 + 1500 => c2 @ 500ms
    assert_eq!(engine.tick_ms(3_001_500), MusicPlaybackState::Playing);
    assert_eq!(engine.current_track_index(), Some(2));
    assert_eq!(engine.position_ms(), 500);
    let snap = engine.snapshot();
    assert_eq!(snap.current_track_id.as_deref(), Some("c2"));
    assert!((snap.progress_ratio - 0.25).abs() < 1e-6);

    // RepeatMode::All with all zero-duration tracks must never infinite-loop
    engine.set_queue(vec![
        MusicTrackEntry::clock_only("z0", "Z0", "A", "B", 0, vec![]),
        MusicTrackEntry::clock_only("z1", "Z1", "A", "B", 0, vec![]),
    ]);
    engine.play().unwrap();
    assert_eq!(engine.tick_ms(500), MusicPlaybackState::Playing);

    // Best-effort OS audio mode MUST NEVER fail play/seek on Linux/headless
    let audio_os = synthesize_music_track_wav(&make_simple_track(SynthWaveformKind::Sine, 1, 0.0))
        .expect("synth os");
    let mut os_engine = MusicTransportEngine::with_os_audio(true);
    os_engine.set_queue(vec![MusicTrackEntry::from_synthesized(
        "os0", "OS Track", "A", "B", audio_os, vec![],
    )]);
    assert!(os_engine.play().is_ok());
    assert_eq!(os_engine.state(), MusicPlaybackState::Playing);
    assert!(os_engine.seek_ms(100).is_ok());
    #[cfg(not(target_os = "macos"))]
    assert!(os_engine.last_os_audio_warning().is_some());
    os_engine.stop();
    os_engine.clear_queue();
    assert!(os_engine.queue().is_empty());
}

#[test]
fn test_adsr_validation_nan_safety_and_chord_case_strictness() {
    // Direct SynthAdsr::amplitude_at guarantees strictly finite output in 0.0..=1.0
    let bad_adsrs = [
        SynthAdsr {
            attack_secs: f32::NAN,
            decay_secs: 0.05,
            sustain_level: 0.7,
            release_secs: 0.1,
        },
        SynthAdsr {
            attack_secs: 0.01,
            decay_secs: f32::INFINITY,
            sustain_level: 0.7,
            release_secs: 0.1,
        },
        SynthAdsr {
            attack_secs: 0.01,
            decay_secs: 0.05,
            sustain_level: f32::NAN,
            release_secs: 0.1,
        },
        SynthAdsr {
            attack_secs: 0.01,
            decay_secs: 0.05,
            sustain_level: 0.7,
            release_secs: f32::INFINITY,
        },
        SynthAdsr {
            attack_secs: -1.0,
            decay_secs: -1.0,
            sustain_level: -5.0,
            release_secs: -1.0,
        },
        SynthAdsr {
            attack_secs: 0.01,
            decay_secs: 0.05,
            sustain_level: 1.5,
            release_secs: 0.1,
        },
        SynthAdsr {
            attack_secs: 0.01,
            decay_secs: 0.05,
            sustain_level: 0.7,
            release_secs: 301.0,
        },
    ];

    for bad in bad_adsrs {
        // Direct non-finite ADSR calls return finite 0.0 (zero NaN poisoning)
        if !bad.attack_secs.is_finite()
            || !bad.decay_secs.is_finite()
            || !bad.sustain_level.is_finite()
            || !bad.release_secs.is_finite()
        {
            let amp = bad.amplitude_at(0.1, 0.5);
            assert!(amp.is_finite() && amp == 0.0, "expected 0.0, got {amp} for {bad:?}");
        } else {
            let amp = bad.amplitude_at(0.1, 0.5);
            assert!(amp.is_finite() && (0.0..=1.0).contains(&amp));
        }

        // Validation rejects all invalid custom ADSR configs
        let mut trk = make_simple_track(SynthWaveformKind::Sine, 1, 0.0);
        trk.instruments[0].adsr = Some(bad);
        assert!(
            synthesize_music_track_wav(&trk).is_err(),
            "expected ADSR rejection for {bad:?}"
        );
    }

    // Valid custom ADSR succeeds
    let mut good_trk = make_simple_track(SynthWaveformKind::Sine, 1, 0.0);
    good_trk.instruments[0].adsr = Some(SynthAdsr {
        attack_secs: 0.01,
        decay_secs: 0.05,
        sustain_level: 0.8,
        release_secs: 0.1,
    });
    assert!(synthesize_music_track_wav(&good_trk).is_ok());

    // Exact chord case sensitivity ("CM" major vs "Cm" minor, "CM6" vs "Cm6")
    assert_eq!(parse_chord_symbol_to_midi("CM", 3), vec![48, 52, 55]);
    assert_eq!(parse_chord_symbol_to_midi("Cm", 3), vec![48, 51, 55]);
    assert_eq!(parse_chord_symbol_to_midi("CM6", 3), vec![48, 52, 55, 57]);
    assert_eq!(parse_chord_symbol_to_midi("Cm6", 3), vec![48, 51, 55, 57]);
    assert_eq!(parse_chord_symbol_to_midi("CM7", 3), vec![48, 52, 55, 59]);
    assert_eq!(parse_chord_symbol_to_midi("Cm7", 3), vec![48, 51, 55, 58]);
    assert_eq!(parse_chord_symbol_to_midi("C#maj7", 3), vec![49, 53, 56, 60]);
    assert_eq!(parse_chord_symbol_to_midi("F#m7b5", 3), vec![54, 57, 60, 64]);
    assert_eq!(parse_chord_symbol_to_midi("Gsus4", 3), vec![55, 60, 62]);

    // Unrecognized chord suffixes & malformed slash bass rejected (empty Vec)
    for bad_chord in [
        "",
        "XYZ",
        "BadChord",
        "Apple",
        "C#invalid",
        "F/invalid",
        "C/Gfoo",
        "C/",
        "C/G/E",
    ] {
        assert!(
            parse_chord_symbol_to_midi(bad_chord, 3).is_empty(),
            "invalid chord unexpectedly parsed: {bad_chord:?}"
        );
    }
}

#[test]
fn test_volume_infinities_u128_tick_oracles_o_n_repeat_all_and_wav_alignment() {
    let mut engine = MusicTransportEngine::new();

    // Volume clamping on +Inf, -Inf, NaN
    engine.set_volume(f32::INFINITY);
    assert_eq!(engine.volume(), 1.0);
    engine.set_volume(f32::NEG_INFINITY);
    assert_eq!(engine.volume(), 0.0);
    engine.set_volume(f32::NAN);
    assert_eq!(engine.volume(), 0.0);

    // Oracle 1: RepeatMode::One, dur=1000, pos=500, tick_ms(u64::MAX) -> pos == 115
    engine.set_queue(vec![MusicTrackEntry::clock_only("o1", "O1", "A", "B", 1000, vec![])]);
    engine.set_repeat_mode(MusicRepeatMode::One);
    engine.play().unwrap();
    engine.seek_ms(500).unwrap();
    assert_eq!(engine.tick_ms(u64::MAX), MusicPlaybackState::Playing);
    assert_eq!(engine.position_ms(), 115);

    // Oracle 2: RepeatMode::One, dur=u64::MAX, pos=500, tick_ms(u64::MAX) -> pos == 500
    engine.set_queue(vec![MusicTrackEntry::clock_only(
        "o2",
        "O2",
        "A",
        "B",
        u64::MAX,
        vec![],
    )]);
    engine.set_repeat_mode(MusicRepeatMode::One);
    engine.play().unwrap();
    engine.seek_ms(500).unwrap();
    assert_eq!(engine.tick_ms(u64::MAX), MusicPlaybackState::Playing);
    assert_eq!(engine.position_ms(), 500);

    // Oracle 3: RepeatMode::All, [1000, 2000], pos=500, tick_ms(u64::MAX) -> idx=Some(1), pos=115
    engine.set_queue(vec![
        MusicTrackEntry::clock_only("a0", "A0", "A", "B", 1000, vec![]),
        MusicTrackEntry::clock_only("a1", "A1", "A", "B", 2000, vec![]),
    ]);
    engine.set_repeat_mode(MusicRepeatMode::All);
    engine.play().unwrap();
    engine.seek_ms(500).unwrap();
    assert_eq!(engine.tick_ms(u64::MAX), MusicPlaybackState::Playing);
    assert_eq!(engine.current_track_index(), Some(1));
    assert_eq!(engine.position_ms(), 115);

    // Oracle 4: RepeatMode::Off, [u64::MAX, 1000], pos=500, tick_ms(u64::MAX) -> idx=Some(1), pos=500
    engine.set_queue(vec![
        MusicTrackEntry::clock_only("f0", "F0", "A", "B", u64::MAX, vec![]),
        MusicTrackEntry::clock_only("f1", "F1", "A", "B", 1000, vec![]),
    ]);
    engine.set_repeat_mode(MusicRepeatMode::Off);
    engine.play().unwrap();
    engine.seek_ms(500).unwrap();
    assert_eq!(engine.tick_ms(u64::MAX), MusicPlaybackState::Playing);
    assert_eq!(engine.current_track_index(), Some(1));
    assert_eq!(engine.position_ms(), 500);

    // Strictly O(N) RepeatMode::All with 4999 zero-duration tracks + 1x1ms track
    let mut big_queue: Vec<MusicTrackEntry> = (0..4999)
        .map(|i| MusicTrackEntry::clock_only(format!("z{i}"), "Z", "A", "B", 0, vec![]))
        .collect();
    big_queue.push(MusicTrackEntry::clock_only("nz", "NZ", "A", "B", 1, vec![]));
    engine.set_queue(big_queue);
    engine.set_repeat_mode(MusicRepeatMode::All);
    engine.play().unwrap();
    let started = Instant::now();
    assert_eq!(engine.tick_ms(1_000_000_000), MusicPlaybackState::Playing);
    let elapsed_ms = started.elapsed().as_millis();
    assert!(
        elapsed_ms < 100,
        "RepeatMode::All tick_ms took {elapsed_ms}ms (expected strictly O(N) < 100ms)"
    );
    assert_eq!(engine.current_track_index(), Some(4999));
    assert_eq!(engine.position_ms(), 0);

    // MusicTrackEntry::from_wav_bytes aligns odd-sample stereo WAV (101 samples, 2 ch)
    let odd_stereo_wav = encode_wav_i16(&[100i16; 101], DEFAULT_SYNTH_SAMPLE_RATE, 2).unwrap();
    let entry = MusicTrackEntry::from_wav_bytes("odd", "Odd", "A", "B", odd_stereo_wav, vec![])
        .expect("odd stereo WAV should frame-align cleanly");
    assert_eq!(entry.pcm_samples.len(), 100);
    assert_eq!(entry.channels, 2);
    engine.set_queue(vec![entry]);
    let sliced = engine
        .sliced_wav_from_position(0)
        .expect("sliced_wav_from_position(0) must succeed after from_wav_bytes");
    let decoded = decode_wav_pcm(&sliced).expect("valid sliced WAV");
    assert_eq!(decoded.samples.len(), 100);

    // 3-channel WAV rejected by from_wav_bytes
    let three_ch_wav = encode_wav_i16(&[100i16; 30], DEFAULT_SYNTH_SAMPLE_RATE, 3).unwrap();
    assert!(
        MusicTrackEntry::from_wav_bytes("ch3", "3ch", "A", "B", three_ch_wav, vec![]).is_err()
    );
}
