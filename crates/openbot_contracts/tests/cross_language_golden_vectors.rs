//! Bit-for-bit cross-language golden parity vectors computed directly by V8 / Node.js
//! against OpenBot `23183891` (`packages/brand/src/bloub-avatar.ts`).

use openbot_contracts::avatar::{
    BloubAvatarProfile, BloubExpression, BloubShape, fnv1a_utf16_32,
};

#[test]
fn exact_v8_utf16_fnv1a_and_bloub_profile_golden_vectors() {
    let hash_vectors: [(&str, u32); 8] = [
        ("", 0x811c_9dc5),
        ("agent-1", 0xe354_67a0),
        ("小龙虾", 0xfada_4a41),
        ("Renée", 0x8942_83a4),
        ("🤖", 0x532b_2b95),
        ("🦞✨", 0x8b80_a35f),
        ("seed-6:shape", 0x0c17_07a8),
        ("seed-6:shape:replacement", 0x9746_a732),
    ];
    for (input, expected) in hash_vectors {
        assert_eq!(
            fnv1a_utf16_32(input),
            expected,
            "UTF-16 FNV-1a mismatch on {input:?}"
        );
    }

    let profile_vectors: [(&str, BloubShape, &str, BloubExpression); 9] = [
        ("", BloubShape::Capsule, "#e8483f", BloubExpression::Excite),
        ("agent-1", BloubShape::Pentagone, "#f08a24", BloubExpression::Fier),
        ("小龙虾", BloubShape::Hexagone, "#2fbfa0", BloubExpression::Triste),
        ("Renée", BloubShape::Cercle, "#3b93f0", BloubExpression::Effraye),
        ("🤖", BloubShape::Nuage, "#e8483f", BloubExpression::Excite),
        ("🦞✨", BloubShape::Pentagone, "#8b5cf6", BloubExpression::Attentif),
        // `seed-12:shape` hashes to index 7 ("goutte") -> remapped via `seed-12:shape:replacement` % 10 -> "carre"
        ("seed-12", BloubShape::Carre, "#3b93f0", BloubExpression::Neutre),
        ("a1:fantome:face:0", BloubShape::Fantome, "#f08a24", BloubExpression::Heureux),
        ("a1:poulpe:flat:7", BloubShape::Poulpe, "#e152b0", BloubExpression::Surpris),
    ];
    for (seed, shape, color, expression) in profile_vectors {
        let actual = BloubAvatarProfile::resolve(seed, None, None);
        assert_eq!(
            actual,
            BloubAvatarProfile {
                shape,
                color: color.to_string(),
                expression,
            },
            "BloubAvatarProfile V8 parity mismatch for seed {seed:?}"
        );
    }
}

#[test]
fn exact_v8_routine_schedule_channel_uuid_and_reaction_golden_vectors() {
    use openbot_contracts::channels::deterministic_create_channel_id;
    use openbot_contracts::conversation::is_message_reaction;
    use openbot_contracts::routines::{
        RoutineSchedule, collapse_missed_occurrences, next_routine_occurrence,
        routine_once_instant, validate_routine_schedule,
    };

    // Exact V8 `routine-schedule.ts` occurrence & DST fold/gap vectors
    let occ_vectors: [(RoutineSchedule, &str, i64, i64); 6] = [
        (
            RoutineSchedule::Custom {
                expression: "0,59 0,23 1 * *".into(),
            },
            "UTC",
            1_767_311_940_000, // 2026-01-01T23:59:00.000Z
            1_769_904_000_000, // 2026-02-01T00:00:00.000Z
        ),
        (
            RoutineSchedule::Custom {
                expression: "0/1 * * * *".into(),
            },
            "UTC",
            1_767_225_600_000, // 2026-01-01T00:00:00.000Z
            1_767_229_200_000, // 2026-01-01T01:00:00.000Z
        ),
        (
            RoutineSchedule::Custom {
                expression: "0 9 15 * 1".into(),
            },
            "UTC",
            1_767_484_800_000, // 2026-01-04T00:00:00.000Z (Sun) -> Mon Jan 5 09:00 via Vixie OR rule
            1_767_603_600_000, // 2026-01-05T09:00:00.000Z
        ),
        (
            RoutineSchedule::Daily {
                time: "02:30".into(),
            },
            "Europe/Warsaw",
            1_774_742_400_000, // 2026-03-29T00:00:00.000Z -> skips 02:30 spring-forward gap to 2026-03-30!
            1_774_830_600_000, // 2026-03-30T00:30:00.000Z
        ),
        (
            RoutineSchedule::Custom {
                expression: "15,45 2 * * *".into(),
            },
            "Europe/Warsaw",
            1_792_887_600_000, // 2026-10-25T00:20:00.000Z (1st 02:20 CEST) -> 1st 02:45 CEST before 2nd 02:15 CET!
            1_792_889_100_000, // 2026-10-25T00:45:00.000Z
        ),
        (
            RoutineSchedule::Custom {
                expression: "15,45 2 * * *".into(),
            },
            "Europe/Warsaw",
            1_792_889_400_000, // 2026-10-25T00:50:00.000Z (1st 02:50 CEST) -> 2nd 02:15 CET!
            1_792_890_900_000, // 2026-10-25T01:15:00.000Z
        ),
    ];
    for (sched, tz, after_ms, expected_ms) in occ_vectors {
        validate_routine_schedule(&sched, tz).expect("valid schedule");
        assert_eq!(
            next_routine_occurrence(&sched, tz, after_ms).unwrap(),
            expected_ms,
            "V8 nextRoutineOccurrence mismatch for {sched:?} in {tz} after {after_ms}"
        );
    }

    assert_eq!(
        routine_once_instant("2026-03-29", "02:30", "Europe/Warsaw").unwrap(),
        1_774_746_000_000 // 2026-03-29T01:00:00.000Z (03:00 CEST)
    );
    assert_eq!(
        collapse_missed_occurrences(
            &RoutineSchedule::Hourly { minute: 15 },
            "UTC",
            1_767_225_600_000,
            1_767_237_600_000,
        )
        .unwrap(),
        (1_767_237_300_000, 1_767_240_900_000)
    );

    assert_eq!(
        deterministic_create_channel_id("turn-1:call-1"),
        "15279afb-9294-8d4a-aa7b-13ef8847a966"
    );
    assert!(is_message_reaction("🏴󠁧󠁢󠁥󠁮󠁧󠁿"));
    assert!(!is_message_reaction("👍🏻🏿"));
    assert!(!is_message_reaction("▰"));
    assert!(!is_message_reaction("🠀"));
}
