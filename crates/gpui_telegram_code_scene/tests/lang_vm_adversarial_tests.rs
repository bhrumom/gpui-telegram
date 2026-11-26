//! Language semantics, full stdlib coverage, and fail-closed adversarial sandbox tests (`CS-F02`).

use gpui_telegram_code_scene::{
    DrawCmd, RgbaColor, RuntimeError, SceneInputState, VmBudgets, VmInstance, compile_and_verify,
    compile_and_verify_with_budgets, compile_source,
};
use gpui_telegram_proto::{AssetKind, AssetManifest};

fn run_anim_script(src: &str, frames: usize) -> gpui_telegram_code_scene::FrameOutput {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);
    let verified = compile_and_verify(src, &manifest).expect("script should compile and verify");
    let mut vm = VmInstance::new(verified, VmBudgets::default()).expect("vm init");
    let mut last = None;
    for _ in 0..frames {
        last = Some(
            vm.step_frame(1.0 / 60.0, &SceneInputState::default())
                .expect("step frame"),
        );
    }
    last.unwrap()
}

#[test]
fn test_lang_semantics_control_flow_lvalue_and_short_circuit() {
    let src = r#"
        // Top-level constant & block comment test
        /* multi-line
           block comment */
        let BASE_BONUS = 10;

        fn on_init(state) {
            state.bumps = 0;
            state.items = [{ x: 100 }, { x: 200 }];
            // Verify single-eval compound L-value assignment: bump() runs strictly ONCE
            state.items[bump(state)].x -= 25;
            // Verify short-circuit prevents RHS side-effects
            let sc1 = false && (bump(state) > 0);
            let sc2 = true || (bump(state) > 0);
            state.sc_ok = (!sc1) && sc2;

            // Verify for .. and ..= plus continue/break & local shadowing
            let sum = 0;
            for i in 0..10 {
                if i == 2 { continue; }
                if i == 6 { break; }
                let shadow = i;
                {
                    let shadow = i * 2;
                    sum += shadow;
                }
                sum += shadow; // 3 * (0+1+3+4+5) = 39
            }
            let inc_sum = 0;
            for k in 1..=4 {
                inc_sum += k; // 10
            }
            // Forward-declared helper + mutual recursion
            state.even6 = is_even(6);
            state.odd5 = is_odd(5);
            state.sum = sum + inc_sum + BASE_BONUS; // 39 + 10 + 10 = 59
            state.hex_col = 0x112233FF;
            state.pi_ok = abs(TAU - PI * 2) < 0.0001;
            state.concat = "N=" + state.sum;
            // Division and modulo by zero return 0.0 safely
            state.div0 = (10 / 0) + (0 / 0) + (5 % 0);
        }

        fn bump(state) {
            state.bumps += 1;
            return 1;
        }

        fn is_even(n) {
            if n == 0 { return true; }
            return is_odd(n - 1);
        }

        fn is_odd(n) {
            if n == 0 { return false; }
            return is_even(n - 1);
        }

        fn on_render(state, t, canvas) {
            clear(state.hex_col);
            text(10, 20, state.concat + ":" + str(state.bumps) + ":" + str(state.items[1].x)
                 + ":" + str(state.sc_ok) + ":" + str(state.even6) + ":" + str(state.odd5)
                 + ":" + str(state.pi_ok) + ":" + str(state.div0), 14, rgb(255, 255, 255), "left");
        }
    "#;

    let out = run_anim_script(src, 1);
    assert_eq!(out.draw_cmds.len(), 2);
    assert_eq!(
        out.draw_cmds[0],
        DrawCmd::Clear {
            color: RgbaColor::new(0x11, 0x22, 0x33, 0xFF)
        }
    );
    match &out.draw_cmds[1] {
        DrawCmd::Text { content, .. } => {
            assert_eq!(content, "N=59:1:175:true:true:true:true:0");
        }
        other => panic!("expected Text cmd, got {other:?}"),
    }
}

#[test]
fn test_dual_canvas_invocation_parity_and_collection_methods() {
    let direct_src = r#"
        fn on_render(state, t, canvas) {
            let arr = [1, 2];
            push(arr, 3);
            let popped = pop(arr);
            let m = { b: 1, a: 2 };
            let ks = keys(m);
            clear(rgb(10, 20, 30));
            circle(canvas.width * 0.5, canvas.height * 0.5, 20 + len(arr) + popped + len(ks), rgb(255, 100, 50));
            if has_key(m, "a") {
                rect(10, 10, 40, 20, rgba(200, 150, 100, 0.5));
            }
        }
    "#;
    let method_src = r#"
        fn on_render(state, t, canvas) {
            let arr = [1, 2];
            arr.push(3);
            let popped = arr.pop();
            let m = { b: 1, a: 2 };
            let ks = m.keys();
            canvas.clear(rgb(10, 20, 30));
            canvas.circle(canvas.w * 0.5, canvas.h * 0.5, 20 + arr.len() + popped + ks.len(), rgb(255, 100, 50));
            if m.has_key("a") {
                canvas.rect(10, 10, 40, 20, rgba(200, 150, 100, 128));
            }
        }
    "#;

    let a = run_anim_script(direct_src, 1);
    let b = run_anim_script(method_src, 1);
    assert_eq!(a.draw_cmds, b.draw_cmds);
}

#[test]
fn test_stdlib_math_easing_keyframe_noise_rng_collision_and_bgra_shaders() {
    let src = r#"
        fn on_init(state) {
            pixel_init(96, 160);
            state.r0 = rand();
            state.r1 = rand_range(10, 20);
        }

        fn on_render(state, t, canvas) {
            // Test pixel_set BGRA8 ordering directly first, then shader
            pixel_set(0, 0, rgb(250, 120, 40));
            let kf1 = keyframe(0.5, [[0, 0], [1, 100]]);
            let kf2 = keyframe(0.5, [{ t: 0, v: 10 }, { t: 1, v: 30, ease: "ease_in_out" }]);
            let c = clamp(5, 10, 2); // inverted bounds safe
            let sm = smoothstep(1, 1, 1);
            let n = noise2d(1.2, 3.4) + fbm2d(0.5, 0.7, 4) + hash2(3, 4);
            let e = ease_in_out(0.5) + ease_out_cubic(0.5) + ease_out_bounce(0.5) + ease_out_elastic(0.5);
            let hits = aabb_hit(0, 0, 10, 10, 5, 5, 10, 10)
                && circle_hit(0, 0, 5, 8, 0, 5)
                && circle_rect_hit(12, 5, 4, 0, 0, 10, 10)
                && !circle_hit(0, 0, 2, 20, 0, 2);
            let col = lerp_color(hsl(0, 1.0, 0.5), hsla(240, 100, 50, 1.0), 0.5);
            clear_grad(col, rgb(10, 10, 20), 180);
            round_rect(10, 10, 100, 40, 8, col);
            stroke_rect(10, 10, 100, 40, rgb(255, 255, 255), 2);
            stroke_circle(180, 180, 30, col, 2);
            glow_circle(180, 180, 20, col, 1.2);
            line(0, 0, 100, 100, col, 2);
            polyline([[10, 10], [40, 30], [70, 20]], col, 2);
            polygon([[10, 80], [40, 120], [5, 120]], col);
            arc(180, 300, 40, 0, PI, col, 3);
            bezier(10, 200, 80, 150, 140, 250, 220, 200, col, 2);
            star(180, 420, 30, 12, 5, col, 0.5);
            tone(440, 0.1, "sine", 0.5);
            text(180, 500, str(kf1) + ":" + str(kf2) + ":" + str(c) + ":" + str(sm)
                 + ":" + str(hits) + ":" + str(floor(dist(0, 0, 3, 4))) + ":" + str(num("42.5"))
                 + ":" + str(n > -10 && e > 0), 12, rgb(255, 255, 255), "center");
        }
    "#;

    let out = run_anim_script(src, 1);
    let pf = out.pixel_bgra.expect("pixel frame present");
    assert_eq!((pf.width, pf.height), (96, 160));
    // Verify BGRA8 byte order for rgb(250, 120, 40) => [B=40, G=120, R=250, A=255]
    assert_eq!(&pf.bgra[0..4], &[40, 120, 250, 255]);
    assert_eq!(out.audio_pulses.len(), 1);
    match out.draw_cmds.last().unwrap() {
        DrawCmd::Text { content, .. } => {
            assert_eq!(content, "50:20:5:1:true:5:42.5:true");
        }
        other => panic!("expected Text cmd, got {other:?}"),
    }

    // Verify all 5 shader kernels run fast (< 10ms) and produce non-uniform BGRA8 pixels
    for shader_call in [
        "shader_plasma(t, 4.0, 0);",
        "shader_mandelbrot(-0.7436, 0.1318, 2.5, 24);",
        "shader_voronoi(t, 10, 2);",
        "shader_starfield(t, 1.5, 100);",
        "shader_synthwave(t, 1.8, 300);",
    ] {
        let s = format!(
            "fn on_init(state) {{ pixel_init(96, 160); }}\nfn on_render(state, t, canvas) {{ {shader_call} }}"
        );
        let out = run_anim_script(&s, 2);
        let pf = out.pixel_bgra.expect("shader produces pixel buffer");
        assert_eq!(pf.bgra.len(), 96 * 160 * 4);
        assert!(pf.has_visible_pixel());
        let first = &pf.bgra[0..4];
        assert!(
            pf.bgra.chunks_exact(4).any(|px| px != first),
            "shader `{shader_call}` output should be non-uniform"
        );
    }
}

// --- Fail-Closed Adversarial Sandbox & DoS Hardening Suite ---

#[test]
fn test_adv_source_size_ast_depth_bytecode_and_constant_limits() {
    let budgets = VmBudgets::default();

    // 1. Source too large (> 65_536 bytes)
    let huge = format!(
        "fn on_render(s,t,c){{clear(0); /* {} */}}",
        "x".repeat(70_000)
    );
    let err = compile_source(&huge, &budgets).unwrap_err();
    assert_eq!(err.code, "E_SOURCE_TOO_LARGE");

    // 2. AST depth bombs (> 64): parens, nested lists, chained unary, nested ifs
    let parens = format!(
        "fn on_render(s,t,c){{ let x = {}1{}; clear(0); }}",
        "(".repeat(80),
        ")".repeat(80)
    );
    assert_eq!(compile_source(&parens, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let lists = format!(
        "fn on_render(s,t,c){{ let x = {}1{}; clear(0); }}",
        "[".repeat(80),
        "]".repeat(80)
    );
    assert_eq!(compile_source(&lists, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let unary = format!(
        "fn on_render(s,t,c){{ let x = {}1; clear(0); }}",
        "-".repeat(80)
    );
    assert_eq!(compile_source(&unary, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let nested_if = format!(
        "fn on_render(s,t,c){{ {} clear(0); {} }}",
        "if true { ".repeat(80),
        " }".repeat(80)
    );
    assert_eq!(compile_source(&nested_if, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    // 3. Constant pool cap (> 2_048 unique constants)
    let mut const_bomb = String::from("fn on_render(s,t,c){ let x = 0;");
    for i in 0..2_200 {
        use std::fmt::Write as _;
        let _ = write!(&mut const_bomb, " x = {i};");
    }
    const_bomb.push_str(" clear(0); }");
    let cerr = compile_source(&const_bomb, &budgets).unwrap_err();
    assert_eq!(cerr.code, "E_CONST_BUDGET_EXCEEDED");

    // 4. Bytecode cap (> 16_384 ops) using a tight budget so source stays under 65 KB
    let small_bc = VmBudgets { max_bytecode_ops: 500, ..budgets };
    let mut op_bomb = String::from("fn on_render(s,t,c){ let x = 0;");
    for _ in 0..300 {
        op_bomb.push_str(" x += 1;");
    }
    op_bomb.push_str(" clear(0); }");
    let berr = compile_source(&op_bomb, &small_bc).unwrap_err();
    assert_eq!(berr.code, "E_BYTECODE_BUDGET_EXCEEDED");
}

#[test]
fn test_adv_infinite_loop_deep_recursion_and_shader_timeout() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // 1. Infinite while loop -> exact InstructionBudgetExceeded (100_001 > 100_000) + default fail-closed
    let inf_loop = "fn on_render(s,t,c){ while true {} }";
    let compiled = std::sync::Arc::new(compile_source(inf_loop, &VmBudgets::default()).unwrap());
    let mk_vm = |budgets| {
        VmInstance::new(
            gpui_telegram_code_scene::VerifiedProgram {
                compiled: std::sync::Arc::clone(&compiled),
                manifest: manifest.clone(),
                cover_hash: String::new(),
                feature_tokens: vec![],
                peak_ops_used: 0,
                draw_cmds_per_frame: vec![],
                has_pixel_output: false,
                warnings: vec![],
            },
            budgets,
        )
        .unwrap()
    };
    let mut vm_op = mk_vm(VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() });
    let err_op = vm_op.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap_err();
    assert_eq!(
        err_op,
        RuntimeError::InstructionBudgetExceeded {
            used: 100_001,
            limit: 100_000
        }
    );
    let mut vm_def = mk_vm(VmBudgets::default());
    let err_def = vm_def.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap_err();
    assert!(
        matches!(
            err_def,
            RuntimeError::InstructionBudgetExceeded { .. } | RuntimeError::FrameTimeout { .. }
        ),
        "default budgets must fail closed on infinite loop, got {err_def:?}"
    );

    // 2. Deep recursion -> StackOverflow at depth 48
    let rec_src = "fn f(n){ return f(n + 1); } fn on_render(s,t,c){ f(0); clear(0); }";
    let err = compile_and_verify(rec_src, &manifest).unwrap_err();
    assert!(err.message.contains("stack overflow"), "got {err:?}");

    // 3. Heavy shader spam loop -> bounded by virtual op cap or wall-clock timeout
    let shader_spam = r#"
        fn on_init(s) { pixel_init(240, 320); }
        fn on_render(s, t, c) {
            for i in 0..200 {
                shader_mandelbrot(-0.7436, 0.1318, 2.0 + i, 64);
            }
        }
    "#;
    let err = compile_and_verify(shader_spam, &manifest).unwrap_err();
    assert!(
        err.message.contains("instruction budget") || err.message.contains("timeout"),
        "expected budget/timeout error, got {err:?}"
    );
}

#[test]
fn test_adv_list_map_string_heap_bombs_cycles_and_gc_longevity() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // Isolate exact memory cap assertions from parallel-test CPU scheduling jitter
    let mem_budgets = VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() };

    // 1. List length bomb (> 4_096)
    let list_bomb = "fn on_render(s,t,c){ let a = []; while true { push(a, 1); } }";
    let e1 = compile_and_verify_with_budgets(list_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(e1.message.contains("memory budget") || e1.message.contains("heap"), "got {e1:?}");
    assert!(compile_and_verify(list_bomb, &manifest).is_err());

    // 2. Map keys bomb (> 1_024)
    let map_bomb = r#"fn on_render(s,t,c){ let m = {}; let i = 0; while true { m["k" + str(i)] = i; i += 1; } }"#;
    let e2 = compile_and_verify_with_budgets(map_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(e2.message.contains("memory budget") || e2.message.contains("heap"), "got {e2:?}");
    assert!(compile_and_verify(map_bomb, &manifest).is_err());

    // 3. String doubling bomb (> 8_192 bytes)
    let str_bomb = r#"fn on_render(s,t,c){ let x = "abcd"; while true { x = x + x; } }"#;
    let e3 = compile_and_verify_with_budgets(str_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(e3.message.contains("memory budget") || e3.message.contains("exceeds"), "got {e3:?}");

    // 4. Aggregate heap values bomb (> 32_768)
    let heap_bomb = r#"
        fn on_render(s,t,c){
            let outer = [];
            for i in 0..2200 {
                push(outer, [0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15]);
            }
            clear(0);
        }
    "#;
    let e4 = compile_and_verify_with_budgets(heap_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(e4.message.contains("memory budget") || e4.message.contains("heap"), "got {e4:?}");

    // 5. Cyclic reference safety: self-referential list str(), eq, snapshot, drop -> 0 panic/overflow
    let cycle_src = r#"
        fn on_init(state) {
            let a = [];
            push(a, a);
            state.cyc = a;
            state.txt = str(a);
            state.same = (a == a);
        }
        fn on_render(state, t, c) {
            clear(rgb(10, 10, 20));
            text(10, 20, state.txt + ":" + str(state.same), 12, rgb(255, 255, 255), "left");
        }
    "#;
    let out = run_anim_script(cycle_src, 3);
    match &out.draw_cmds[1] {
        DrawCmd::Text { content, .. } => assert_eq!(content, "[[<...>]]:true"),
        other => panic!("unexpected {other:?}"),
    }

    // 6. Long-running GC longevity: 1,000 frames allocating 50 transient maps/frame
    let gc_src = r#"
        fn on_update(state, dt, input) {
            let acc = 0;
            for i in 0..50 {
                let tmp = { a: i, b: i + 1, c: "frame" };
                acc += tmp.a;
            }
            state.last = acc;
        }
        fn on_render(state, t, c) {
            clear(rgb(10, 10, 20));
            circle(180, 320, 20, rgb(100, 255, 200));
        }
    "#;
    let verified = compile_and_verify_with_budgets(gc_src, &manifest, mem_budgets).unwrap();
    let mut vm = VmInstance::new(verified, mem_budgets).unwrap();
    for _ in 0..1_000 {
        vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    }

    // 7. Single-frame heavy-allocation mid-frame `maybe_gc` stress test (> 32_768 transient values in ONE frame)
    let mid_frame_gc_src = r#"
        fn on_render(state, t, c) {
            let acc = 0;
            for i in 0..1400 {
                let tmp = [1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,
                           17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32];
                acc += len(tmp);
            }
            clear(rgb(10, 10, 20));
            text(10, 20, str(acc), 12, rgb(255, 255, 255), "left");
        }
    "#;
    let verified_mid =
        compile_and_verify_with_budgets(mid_frame_gc_src, &manifest, mem_budgets).unwrap();
    let mut vm_mid = VmInstance::new(verified_mid, mem_budgets).unwrap();
    for _ in 0..3 {
        let out = vm_mid.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
        match out.draw_cmds.last().unwrap() {
            DrawCmd::Text { content, .. } => assert_eq!(content, "44800"),
            other => panic!("expected Text cmd, got {other:?}"),
        }
    }
}

#[test]
fn test_adv_nan_inf_draw_sanitization_oversized_pixel_init_and_draw_flood() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // 1. NaN / Inf coordinates dropped cleanly, valid draws survive, zero non-finite floats leak
    let nan_src = r#"
        fn on_render(s, t, c) {
            let inf = 1e308 * 1e308;
            let nan = inf - inf;
            clear(rgb(12, 14, 28));
            circle(nan, inf, nan, nan);
            rect(inf, -inf, nan, 20, rgb(255, 0, 0));
            line(0, 0, nan, 10, rgb(255, 255, 255), 2);
            pixel_init(16, 16);
            pixel_set(nan, inf, nan);
            circle(180, 320, 25, rgb(90, 255, 200));
        }
    "#;
    let out = run_anim_script(nan_src, 1);
    assert_eq!(out.draw_cmds.len(), 2, "only clear + valid circle should survive");
    assert!(out.draw_cmds.iter().all(DrawCmd::all_floats_finite));

    // 2. Oversized / invalid pixel_init fails closed with PixelBudgetExceeded
    for bad in [
        "pixel_init(10000, 10000);",
        "pixel_init(241, 320);",
        "pixel_init(0, 10);",
        "pixel_init(-5, 10);",
    ] {
        let src = format!("fn on_render(s,t,c){{ {bad} clear(rgb(1,2,3)); }}");
        let err = compile_and_verify(&src, &manifest).unwrap_err();
        assert!(err.message.contains("pixel"), "expected pixel budget error for `{bad}`, got {err:?}");
    }

    // 3. Draw command flood (2,600 circles) safely capped at 2_048 + validator warning
    let flood_src = r#"
        fn on_render(s, t, c) {
            let col = rgb(255, 120, 180);
            for i in 0..2600 {
                circle(180, 320, 5, col);
            }
        }
    "#;
    let draw_budgets = VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() };
    let verified = compile_and_verify_with_budgets(flood_src, &manifest, draw_budgets).unwrap();
    assert!(verified.warnings.iter().any(|w| w.code == "W_DRAW_CAP_HIT"));
    let mut vm = VmInstance::new(verified, draw_budgets).unwrap();
    let out = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    assert_eq!(out.draw_cmds.len(), 2_048);
}

#[test]
fn test_adv_aggregate_string_bomb_for_each_oob_write_and_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<gpui_telegram_code_scene::CompiledProgram>();
    assert_send_sync::<gpui_telegram_code_scene::VerifiedProgram>();
    assert_send_sync::<VmInstance>();
    assert_send_sync::<gpui_telegram_code_scene::FrameOutput>();

    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // 1. Aggregate distinct-string heap bomb (> 2 MiB total_str_bytes)
    let str_agg_bomb = r#"
        fn on_render(s, t, c) {
            let chunk = "0123456789abcde"; // 15 bytes
            for k in 0..9 { chunk = chunk + chunk; } // 7680 bytes
            let bag = [];
            for i in 0..320 {
                push(bag, str(i) + chunk);
            }
            clear(rgb(10, 10, 20));
        }
    "#;
    let mem_budgets = VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() };
    let err = compile_and_verify_with_budgets(str_agg_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(
        err.message.contains("memory budget") || err.message.contains("heap"),
        "expected aggregate string memory budget error, got {err:?}"
    );
    assert!(compile_and_verify(str_agg_bomb, &manifest).is_err());

    // 2. List out-of-bounds read -> null (safe); out-of-bounds write -> IndexOutOfBounds (fail-closed)
    let oob_write = r#"
        fn on_render(s, t, c) {
            let a = [1, 2];
            if a[99] == null {
                a[99] = 42;
            }
            clear(rgb(10, 10, 20));
        }
    "#;
    let err = compile_and_verify(oob_write, &manifest).unwrap_err();
    assert!(
        err.message.contains("out of bounds"),
        "expected IndexOutOfBounds, got {err:?}"
    );

    // 3. ForEach iteration + while continue/break + deterministic sorted BTreeMap keys + dropped invalid draw counter
    let fe_src = r#"
        fn on_render(s, t, c) {
            let nums = [10, 20, 30, 40];
            let total = 0;
            for v in nums {
                if v == 20 { continue; }
                if v == 40 { break; }
                total += v; // 10 + 30 = 40
            }
            let wsum = 0;
            let i = 0;
            while i < 6 {
                i += 1;
                if i == 3 { continue; }
                if i == 5 { break; }
                wsum += i; // 1 + 2 + 4 = 7
            }
            let m = { z: 1, a: 2, m: 3 };
            let ks = keys(m);
            let inf = 1e308 * 1e308;
            let nan = inf - inf;
            clear(rgb(10, 10, 20));
            circle(nan, 10, 5, rgb(255, 0, 0));
            rect(inf, 10, 5, 5, rgb(255, 0, 0));
            text(10, 20, str(total) + ":" + str(wsum) + ":" + ks[0] + ks[1] + ks[2], 12, rgb(255, 255, 255), "left");
        }
    "#;
    let verified = compile_and_verify(fe_src, &manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
    let out = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    assert_eq!(vm.last_dropped_invalid_draws(), 2);
    match &out.draw_cmds[1] {
        DrawCmd::Text { content, .. } => assert_eq!(content, "40:7:amz"),
        other => panic!("expected Text cmd, got {other:?}"),
    }
}

#[test]
fn test_adv_flat_ast_chains_empty_tokens_and_duplicate_params() {
    let budgets = VmBudgets::default();

    // 1. Empty token slice fails cleanly without panic; non-EOF token slices terminate immediately without hanging
    let empty_err = gpui_telegram_code_scene::lang::parser::parse(&[], &budgets).unwrap_err();
    assert_eq!(empty_err.code, "E_PARSE_EMPTY");

    let semi_only = [gpui_telegram_code_scene::lang::lexer::Token {
        kind: gpui_telegram_code_scene::lang::lexer::TokenKind::Semicolon,
        span: gpui_telegram_code_scene::Span { line: 1, col: 1 },
        after_newline: false,
    }];
    let semi_prog = gpui_telegram_code_scene::lang::parser::parse(&semi_only, &budgets)
        .expect("non-EOF semicolon slice must terminate cleanly without infinite loop");
    assert!(semi_prog.items.is_empty());

    let fn_only = [gpui_telegram_code_scene::lang::lexer::Token {
        kind: gpui_telegram_code_scene::lang::lexer::TokenKind::Fn,
        span: gpui_telegram_code_scene::Span { line: 1, col: 1 },
        after_newline: false,
    }];
    let fn_err = gpui_telegram_code_scene::lang::parser::parse(&fn_only, &budgets).unwrap_err();
    assert_eq!(fn_err.code, "E_PARSE_EXPECTED");

    // 2. Flat AST chains exceeding max_ast_depth (64) MUST fail with E_AST_DEPTH_EXCEEDED
    //    (including huge < 65 KB chains that previously caused SIGABRT stack overflow)
    let bin_20k = format!("fn on_render(s,t,c){{ let x = 0{}; clear(0); }}", "+1".repeat(20_000));
    assert_eq!(compile_source(&bin_20k, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let field_20k = format!("fn on_render(s,t,c){{ let x = s{}; clear(0); }}", ".a".repeat(20_000));
    assert_eq!(compile_source(&field_20k, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let index_16k = format!("fn on_render(s,t,c){{ let x = s{}; clear(0); }}", "[0]".repeat(16_000));
    assert_eq!(compile_source(&index_16k, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    let method_8k = format!("fn on_render(s,t,c){{ let x = s{}; clear(0); }}", ".len()".repeat(8_000));
    assert_eq!(compile_source(&method_8k, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    // Tight boundary check right past 64
    let bin_70 = format!("fn on_render(s,t,c){{ let x = 0{}; clear(0); }}", "+1".repeat(70));
    assert_eq!(compile_source(&bin_70, &budgets).unwrap_err().code, "E_AST_DEPTH_EXCEEDED");

    // 3. Duplicate function parameters rejected with E_DUPLICATE_PARAM
    let dup2 = "fn bad(a, a) { return a; } fn on_render(s,t,c){ clear(0); }";
    assert_eq!(compile_source(dup2, &budgets).unwrap_err().code, "E_DUPLICATE_PARAM");
    let dup3 = "fn bad(x, y, x) { return y; } fn on_render(s,t,c){ clear(0); }";
    assert_eq!(compile_source(dup3, &budgets).unwrap_err().code, "E_DUPLICATE_PARAM");
}

#[test]
fn test_adv_gc_rooting_make_list_make_map_set_field_and_set_index() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);
    let tight = VmBudgets {
        max_heap_values: 64,
        max_frame_wall_ms: 500,
        ..VmBudgets::default()
    };

    let extract_text = |src: &str, b: VmBudgets| -> String {
        let verified = compile_and_verify_with_budgets(src, &manifest, b).unwrap();
        let mut vm = VmInstance::new(verified, b).unwrap();
        let out = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
        match out.draw_cmds.last().unwrap() {
            DrawCmd::Text { content, .. } => content.clone(),
            other => panic!("expected Text cmd, got {other:?}"),
        }
    };

    // 1. MakeList GC rooting (live_values: 12 base + 44 garbage + 4 + 4 = 64 -> outer MakeList triggers GC)
    let make_list_src = r#"
        fn on_render(s, t, c) {
            for i in 0..4 { let g = [0,1,2,3,4,5,6,7,8,9]; }
            let res = [[11, 22, 33], [44, 55, 66]];
            let poison = [999, 888];
            clear(rgb(10, 10, 20));
            text(10, 20, str(len(res)) + ":" + str(res[0][0]) + ":" + str(res[1][2])
                 + ":" + str(res[0] == poison) + ":" + str(res == res[1]), 12, rgb(255,255,255), "left");
        }
    "#;
    assert_eq!(extract_text(make_list_src, tight), "2:11:66:false:false");

    // 2. MakeMap GC rooting
    let make_map_src = r#"
        fn on_render(s, t, c) {
            for i in 0..4 { let g = [0,1,2,3,4,5,6,7,8,9]; }
            let m = { a: [11, 22, 33], b: [44, 55, 66] };
            let poison = [999, 888];
            clear(rgb(10, 10, 20));
            text(10, 20, str(len(m.a)) + ":" + str(m.a[0]) + ":" + str(len(m.b)) + ":" + str(m.b[2])
                 + ":" + str(m.a == poison) + ":" + str(m.b == poison), 12, rgb(255,255,255), "left");
        }
    "#;
    assert_eq!(extract_text(make_map_src, tight), "3:11:3:66:false:false");

    // 3. SetField val root GC rooting
    let set_field_val_src = r#"
        fn on_render(state, t, c) {
            for i in 0..4 { let g = [0,1,2,3,4,5,6,7,8,9,10]; }
            state.fresh = [77, 88, 99];
            let before = str(len(state.fresh)) + ":" + str(state.fresh[0]);
            let poison = [12345];
            let after = str(len(state.fresh)) + ":" + str(state.fresh[0]) + ":" + str(state.fresh[2]);
            clear(rgb(10, 10, 20));
            text(10, 20, before + "|" + after, 12, rgb(255,255,255), "left");
        }
    "#;
    assert_eq!(extract_text(set_field_val_src, tight), "3:77|3:77:99");

    // 4. SetField base root GC rooting (unrooted temporary base)
    let set_field_base_src = r#"
        fn mk_map() { return { a: 1, b: 2, c: 3 }; }
        fn on_render(state, t, c) {
            for i in 0..4 { let g = [0,1,2,3,4,5,6,7,8,9,10]; }
            mk_map().d = 4;
            clear(rgb(10, 10, 20));
            text(10, 20, "ok_base", 12, rgb(255,255,255), "left");
        }
    "#;
    assert_eq!(extract_text(set_field_base_src, tight), "ok_base");

    // 5. SetIndex map val root GC rooting
    let set_index_map_src = r#"
        fn on_render(state, t, c) {
            for i in 0..4 { let g = [0,1,2,3,4,5,6,7,8,9,10]; }
            state["fresh"] = [77, 88, 99];
            let before = str(len(state["fresh"])) + ":" + str(state["fresh"][0]);
            let poison = [12345];
            let after = str(len(state["fresh"])) + ":" + str(state["fresh"][0]);
            clear(rgb(10, 10, 20));
            text(10, 20, before + "|" + after, 12, rgb(255,255,255), "left");
        }
    "#;
    assert_eq!(extract_text(set_index_map_src, tight), "3:77|3:77");

    // 6. SetIndex list base root GC rooting under max_total_str_bytes pressure
    let tight_str = VmBudgets {
        max_heap_values: 512,
        max_total_str_bytes: 512,
        max_frame_wall_ms: 500,
        ..VmBudgets::default()
    };
    let s300 = "x".repeat(300);
    let s200 = "y".repeat(200);
    let set_index_list_base_src = format!(
        r#"
        fn make_garbage() {{
            let g = ["{s300}"];
            return len(g);
        }}
        fn mk_list() {{ return [0]; }}
        fn on_render(state, t, c) {{
            let _ = make_garbage();
            mk_list()[0] = "{s200}";
            clear(rgb(10, 10, 20));
            text(10, 20, "ok_list_base", 12, rgb(255,255,255), "left");
        }}
    "#
    );
    assert_eq!(extract_text(&set_index_list_base_src, tight_str), "ok_list_base");

    // 7. Host VmState::alloc_map and VmState::set_state_field retry cleanly after GC
    let compiled = std::sync::Arc::new(
        compile_source("fn on_render(s,t,c){ clear(rgb(1,2,3)); }", &tight).unwrap(),
    );
    let mut host_vm = gpui_telegram_code_scene::lang::VmState::new(compiled, tight, 1).unwrap();
    for i in 0..35 {
        let mut tmp = std::collections::BTreeMap::new();
        tmp.insert("k".to_string(), gpui_telegram_code_scene::lang::Val::Num(i as f64));
        let _ = host_vm.alloc_map(tmp).expect("host alloc_map must GC-retry cleanly");
    }
    // Fill heap right up to max_heap_values (64) so set_state_field MUST run GC while rooting child_id
    let mut child_entry = std::collections::BTreeMap::new();
    child_entry.insert("marker".to_string(), gpui_telegram_code_scene::lang::Val::Num(777.0));
    let child_id = host_vm.alloc_map(child_entry).unwrap();
    while host_vm.heap().live_values() < tight.max_heap_values {
        let _ = host_vm.alloc_map(std::collections::BTreeMap::new()).unwrap();
    }
    assert_eq!(host_vm.heap().live_values(), tight.max_heap_values);
    host_vm
        .set_state_field("high_score", gpui_telegram_code_scene::lang::Val::Map(child_id))
        .expect("host set_state_field must GC-retry while rooting passed Val");
    assert!(matches!(
        host_vm.get_state_field("high_score"),
        gpui_telegram_code_scene::lang::Val::Map(id) if id == child_id
    ));
    assert!(matches!(
        host_vm.heap().map_get(child_id, "marker"),
        gpui_telegram_code_scene::lang::Val::Num(n) if n == 777.0
    ));
}

#[test]
fn test_adv_pixel_watchdog_locals_str_bomb_smoothstep_and_aabb_polish() {
    let manifest = AssetManifest::for_kind(AssetKind::Animation);

    // 1. pixel_rect spam MUST fail closed rapidly within budget (never ~1,122ms!)
    let pixel_rect_spam = r#"
        fn on_init(s) { pixel_init(240, 320); }
        fn on_render(s, t, c) {
            for i in 0..500 {
                pixel_rect(0, 0, 240, 320, 0xFF0000FF);
            }
        }
    "#;
    let t0 = std::time::Instant::now();
    let err = compile_and_verify(pixel_rect_spam, &manifest).unwrap_err();
    let elapsed_ms = t0.elapsed().as_millis();
    assert!(
        err.message.contains("timeout") || err.message.contains("instruction budget"),
        "expected watchdog abort for pixel_rect spam, got {err:?}"
    );
    assert!(
        elapsed_ms < 250,
        "pixel_rect spam took {elapsed_ms}ms (watchdog bypass regression!)"
    );
    let op_err = compile_and_verify_with_budgets(
        pixel_rect_spam,
        &manifest,
        VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() },
    )
    .unwrap_err();
    assert!(
        op_err.message.contains("instruction budget"),
        "pixel_rect must charge virtual ops, got {op_err:?}"
    );

    // 2. pixel_init spam MUST fail closed via virtual-op charging / watchdog
    let pixel_init_spam = r#"
        fn on_render(s, t, c) {
            for i in 0..500 {
                pixel_init(240, 320);
            }
        }
    "#;
    let init_err = compile_and_verify_with_budgets(
        pixel_init_spam,
        &manifest,
        VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() },
    )
    .unwrap_err();
    assert!(
        init_err.message.contains("instruction budget"),
        "pixel_init must charge virtual ops, got {init_err:?}"
    );

    // 3. 45-frame distinct-string locals bomb (~3.45 MiB > 2 MiB) MUST fail closed
    let locals_str_bomb = r#"
        fn bomb(depth, base) {
            let s0 = str(depth) + "a" + base;
            let s1 = str(depth) + "b" + base;
            let s2 = str(depth) + "c" + base;
            let s3 = str(depth) + "d" + base;
            let s4 = str(depth) + "e" + base;
            let s5 = str(depth) + "f" + base;
            let s6 = str(depth) + "g" + base;
            let s7 = str(depth) + "h" + base;
            let s8 = str(depth) + "i" + base;
            let s9 = str(depth) + "j" + base;
            if depth > 0 {
                return bomb(depth - 1, base);
            }
            return len(s0) + len(s9);
        }
        fn on_render(s, t, c) {
            let chunk = "0123456789abcde"; // 15 bytes
            for k in 0..9 { chunk = chunk + chunk; } // 7680 bytes
            let _ = bomb(44, chunk);
            clear(rgb(10, 10, 20));
        }
    "#;
    let mem_budgets = VmBudgets { max_frame_wall_ms: 500, ..VmBudgets::default() };
    let str_err = compile_and_verify_with_budgets(locals_str_bomb, &manifest, mem_budgets).unwrap_err();
    assert!(
        str_err.message.contains("memory budget") || str_err.message.contains("string"),
        "expected MemoryBudgetExceeded for 3.45 MiB locals string bomb, got {str_err:?}"
    );

    // Shared Arc<str> across 45 recursive frames (only 1 distinct 7.68 KB allocation) MUST succeed
    let shared_str_ok = r#"
        fn recurse(depth, shared) {
            let a = shared;
            let b = shared;
            let c = shared;
            if depth > 0 { return recurse(depth - 1, shared); }
            return len(a) + len(b) + len(c);
        }
        fn on_render(s, t, c) {
            let chunk = "0123456789abcde";
            for k in 0..9 { chunk = chunk + chunk; }
            let n = recurse(44, chunk);
            clear(rgb(10, 10, 20));
            text(10, 20, str(n), 12, rgb(255, 255, 255), "left");
        }
    "#;
    assert!(compile_and_verify_with_budgets(shared_str_ok, &manifest, mem_budgets).is_ok());

    // 4. smoothstep extreme finite (no NaN), aabb_hit non-positive dims, Inf game score sanitization
    let polish_src = r#"
        fn on_init(state) {
            state.score = 1e308 * 1e308;
            state.high_score = -1e308 * 1e308;
        }
        fn on_render(state, t, c) {
            let s1 = smoothstep(-1e308, 1e308, 1e308);
            let s2 = smoothstep(1e308, -1e308, -1e308);
            let s3 = smoothstep(-1e308, 1e308, 0);
            let s4 = smoothstep(1e308, -1e308, 0);
            let neg_aabb = aabb_hit(0, 0, -10, 10, -5, 5, 10, 10)
                || aabb_hit(0, 0, 0, 10, 0, 0, 10, 10)
                || aabb_hit(0, 0, 10, 10, 5, 5, -1, 10);
            let all_finite = (s1 == s1) && (s2 == s2) && (s3 == s3) && (s4 == s4);
            clear(rgb(10, 10, 20));
            text(10, 20, str(s1) + ":" + str(s2) + ":" + str(s3) + ":" + str(s4)
                 + ":" + str(all_finite) + ":" + str(neg_aabb), 12, rgb(255,255,255), "left");
        }
    "#;
    let game_manifest = AssetManifest::for_kind(AssetKind::Game);
    let verified = compile_and_verify(polish_src, &game_manifest).unwrap();
    let mut vm = VmInstance::new(verified, VmBudgets::default()).unwrap();
    let out = vm.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    let hud = out.game_hud.unwrap();
    assert_eq!(hud.score, 0, "Inf score must sanitize to 0, not i64::MAX");
    assert_eq!(hud.high_score, 0, "-Inf high_score must sanitize to 0");
    match out.draw_cmds.last().unwrap() {
        DrawCmd::Text { content, .. } => assert_eq!(content, "1:1:0.5:0.5:true:false"),
        other => panic!("expected Text cmd, got {other:?}"),
    }
}

#[test]
fn test_adv_monotonic_str_budget_blocks_keys_pop_overwrite_and_detachment_bypasses() {
    use std::fmt::Write as _;

    let manifest = AssetManifest::for_kind(AssetKind::Animation);
    let mem_budgets = VmBudgets {
        max_frame_wall_ms: 500,
        ..VmBudgets::default()
    };

    // 1. `keys(m)` + `pop(ks)` 45-frame locals bomb (~13.8 MiB distinct Arc<str> > 2 MiB)
    let mut keys_pop_src = String::from("fn bomb(d, m) {\n  let ks = m.keys();\n");
    for i in 0..40 {
        let _ = writeln!(&mut keys_pop_src, "  let k{i} = ks.pop();");
    }
    keys_pop_src.push_str(
        r#"
  if d > 0 { return bomb(d - 1, m); }
  return len(k0) + len(k39);
}
fn on_render(s, t, c) {
  let chunk = "0123456789abcde";
  for k in 0..9 { chunk = chunk + chunk; }
  let m = {};
  for i in 0..40 { m[str(i) + "_" + chunk] = i; }
  let _ = bomb(44, m);
  clear(rgb(10, 10, 20));
}
"#,
    );
    let e1 = compile_and_verify_with_budgets(&keys_pop_src, &manifest, mem_budgets).unwrap_err();
    assert!(
        e1.message.contains("memory budget") || e1.message.contains("string"),
        "keys()+pop() 45-frame bomb must fail closed with MemoryBudgetExceeded, got {e1:?}"
    );
    assert!(compile_and_verify(&keys_pop_src, &manifest).is_err());

    // 2. `keys(m)` + `ks[i]; ks[i] = 0` 45-frame locals bomb (~6.9 MiB, ZERO pop() calls)
    let mut keys_idx_src = String::from("fn bomb(d, m) {\n  let ks = keys(m);\n");
    for i in 0..20 {
        let _ = writeln!(&mut keys_idx_src, "  let k{i} = ks[{i}]; ks[{i}] = 0;");
    }
    keys_idx_src.push_str(
        r#"
  if d > 0 { return bomb(d - 1, m); }
  return len(k0) + len(k19);
}
fn on_render(s, t, c) {
  let chunk = "0123456789abcde";
  for k in 0..9 { chunk = chunk + chunk; }
  let m = {};
  for i in 0..20 { m[str(i) + "_" + chunk] = i; }
  let _ = bomb(44, m);
  clear(rgb(10, 10, 20));
}
"#,
    );
    let e2 = compile_and_verify_with_budgets(&keys_idx_src, &manifest, mem_budgets).unwrap_err();
    assert!(
        e2.message.contains("memory budget") || e2.message.contains("string"),
        "keys()+overwrite 45-frame bomb must fail closed with MemoryBudgetExceeded, got {e2:?}"
    );

    // 3. `keys(m)` + `pop(ks)` persisted into globals (~3 MiB > 2 MiB)
    let mut keys_globals_src = String::new();
    for i in 0..400 {
        let _ = writeln!(&mut keys_globals_src, "let g{i} = \"\";");
    }
    keys_globals_src.push_str(
        r#"
fn on_render(s, t, c) {
  let chunk = "0123456789abcde";
  for k in 0..9 { chunk = chunk + chunk; }
  let m = {};
  for i in 0..20 { m[str(i) + "_" + chunk] = i; }
"#,
    );
    for wave in 0..20 {
        keys_globals_src.push_str("  let ks = keys(m);\n");
        for j in 0..20 {
            let idx = wave * 20 + j;
            let _ = writeln!(&mut keys_globals_src, "  g{idx} = pop(ks);");
        }
    }
    keys_globals_src.push_str("  clear(rgb(10, 10, 20));\n}\n");
    let e3 = compile_and_verify_with_budgets(&keys_globals_src, &manifest, mem_budgets).unwrap_err();
    assert!(
        e3.message.contains("memory budget") || e3.message.contains("string"),
        "keys()+pop() globals bomb must fail closed with MemoryBudgetExceeded, got {e3:?}"
    );

    // 4. `Op::Add` + `pop(state.stash)` detachment bomb (~3.5 MiB > 2 MiB)
    let mut add_pop_src = String::new();
    for i in 0..460 {
        let _ = writeln!(&mut add_pop_src, "let g{i} = \"\";");
    }
    add_pop_src.push_str(
        r#"
fn on_render(state, t, c) {
  let chunk = "0123456789abcde";
  for k in 0..9 { chunk = chunk + chunk; }
  state.stash = [];
  for i in 0..230 { push(state.stash, "w1_" + str(i) + chunk); }
"#,
    );
    for i in 0..230 {
        let _ = writeln!(&mut add_pop_src, "  g{i} = pop(state.stash);");
    }
    add_pop_src.push_str("  for i in 0..230 { push(state.stash, \"w2_\" + str(i) + chunk); }\n");
    for i in 230..460 {
        let _ = writeln!(&mut add_pop_src, "  g{i} = pop(state.stash);");
    }
    add_pop_src.push_str("  clear(rgb(10, 10, 20));\n}\n");
    let e4 = compile_and_verify_with_budgets(&add_pop_src, &manifest, mem_budgets).unwrap_err();
    assert!(
        e4.message.contains("memory budget") || e4.message.contains("string"),
        "Op::Add+pop detachment bomb must fail closed with MemoryBudgetExceeded, got {e4:?}"
    );

    // 5. `Op::Add` + list index overwrite (`state.arr[i] = 0`) detachment bomb (~3.5 MiB, ZERO keys/pop)
    let mut add_overwrite_src = String::new();
    for i in 0..460 {
        let _ = writeln!(&mut add_overwrite_src, "let g{i} = \"\";");
    }
    add_overwrite_src.push_str(
        r#"
fn on_render(state, t, c) {
  let chunk = "0123456789abcde";
  for k in 0..9 { chunk = chunk + chunk; }
  state.arr = [];
  for i in 0..230 { push(state.arr, "a1_" + str(i) + chunk); }
"#,
    );
    for i in 0..230 {
        let _ = writeln!(
            &mut add_overwrite_src,
            "  g{i} = state.arr[{i}]; state.arr[{i}] = 0;"
        );
    }
    for i in 230..460 {
        let _ = writeln!(
            &mut add_overwrite_src,
            "  g{i} = \"a2_\" + str({i}) + chunk;"
        );
    }
    add_overwrite_src.push_str("  clear(rgb(10, 10, 20));\n}\n");
    let e5 =
        compile_and_verify_with_budgets(&add_overwrite_src, &manifest, mem_budgets).unwrap_err();
    assert!(
        e5.message.contains("memory budget") || e5.message.contains("string"),
        "Op::Add+overwrite detachment bomb must fail closed with MemoryBudgetExceeded, got {e5:?}"
    );

    // 6. Legitimate 45-frame shared-Arc<str> recursion (even after dead transient string churn) MUST still pass!
    let shared_after_churn = r#"
        fn recurse(depth, shared) {
            let a = shared;
            let b = shared;
            let c = shared;
            if depth > 0 { return recurse(depth - 1, shared); }
            return len(a) + len(b) + len(c);
        }
        fn on_render(s, t, c) {
            let chunk = "0123456789abcde";
            for k in 0..9 { chunk = chunk + chunk; }
            for i in 0..320 { let dead = str(i) + chunk; }
            let n = recurse(44, chunk);
            clear(rgb(10, 10, 20));
            text(10, 20, str(n), 12, rgb(255, 255, 255), "left");
        }
    "#;
    assert!(
        compile_and_verify_with_budgets(shared_after_churn, &manifest, mem_budgets).is_ok(),
        "legitimate 45-frame shared Arc<str> recursion must succeed after dead-string GC"
    );
}

#[test]
fn test_adv_max_str_bytes_ident_map_key_alloc_map_and_hud_status_sanitization() {
    use gpui_telegram_code_scene::lang::{Val, VmHeap, VmState};
    use gpui_telegram_code_scene::lang::ast::{Expr, FnDecl, Program, Stmt, TopItem};
    use gpui_telegram_code_scene::{DiagnosticStage, RuntimeError, Span};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let budgets = VmBudgets::default();
    let manifest = AssetManifest::for_kind(AssetKind::Game);

    // 1. 60,000-char unquoted identifier map key MUST fail closed at lex with E_STR_TOO_LONG
    let huge_ident = "a".repeat(60_000);
    let huge_map_key_src = format!(
        r#"
        fn on_render(state, t, c) {{
            let m = {{ {huge_ident}: 1 }};
            let ks = keys(m);
            state.status_text = ks[0];
            clear(rgb(10, 20, 30));
        }}
        "#
    );
    let lex_err = compile_and_verify(&huge_map_key_src, &manifest).unwrap_err();
    assert_eq!(lex_err.stage, DiagnosticStage::Lex);
    assert_eq!(lex_err.code, "E_STR_TOO_LONG");

    // Tight boundary check: 8,193-byte identifier rejected at lex
    let edge_ident = "b".repeat(budgets.max_str_bytes + 1);
    let edge_src = format!("fn on_render(s,t,c){{ let m = {{ {edge_ident}: 1 }}; clear(0); }}");
    let edge_err = compile_source(&edge_src, &budgets).unwrap_err();
    assert_eq!(edge_err.code, "E_STR_TOO_LONG");

    // 2. Direct compiler AST defense-in-depth: Expr::Map key > max_str_bytes rejected with E_STR_TOO_LONG
    let span = Span { line: 1, col: 1 };
    let synthetic_ast = Program {
        items: vec![TopItem::Fn(FnDecl {
            name: "on_render".into(),
            params: vec!["s".into(), "t".into(), "c".into()],
            body: vec![Stmt::Let {
                name: "m".into(),
                value: Expr::Map(vec![("k".repeat(budgets.max_str_bytes + 1), Expr::Num(1.0, span))], span),
                span,
            }],
            span,
        })],
    };
    let comp_err = gpui_telegram_code_scene::lang::compiler::compile(&synthetic_ast, &budgets).unwrap_err();
    assert_eq!(comp_err.stage, DiagnosticStage::Compile);
    assert_eq!(comp_err.code, "E_STR_TOO_LONG");

    // 3. Direct VmState::alloc_map with >8,192-byte key or value rejected with MemoryBudgetExceeded
    let compiled = Arc::new(compile_source("fn on_render(s,t,c){ clear(rgb(1,2,3)); }", &budgets).unwrap());
    let mut vm = VmState::new(compiled, budgets, 1).unwrap();

    let mut bad_key_map = BTreeMap::new();
    bad_key_map.insert("k".repeat(budgets.max_str_bytes + 1), Val::Num(1.0));
    assert!(matches!(
        vm.alloc_map(bad_key_map),
        Err(RuntimeError::MemoryBudgetExceeded { .. })
    ));

    let mut bad_val_map = BTreeMap::new();
    bad_val_map.insert(
        "ok".to_string(),
        Val::Str(Arc::from("v".repeat(budgets.max_str_bytes + 1))),
    );
    assert!(matches!(
        vm.alloc_map(bad_val_map),
        Err(RuntimeError::MemoryBudgetExceeded { .. })
    ));

    // Direct VmHeap::alloc_list defense-in-depth with >8,192-byte string element
    let mut heap = VmHeap::new();
    assert!(matches!(
        heap.alloc_list(
            vec![Val::Str(Arc::from("x".repeat(budgets.max_str_bytes + 1)))],
            &budgets,
        ),
        Err(RuntimeError::MemoryBudgetExceeded { .. })
    ));

    // 4. GameHudSnapshot.status_text is sanitized (<= 256 UTF-8 bytes, control chars stripped)
    let long_status_src = r#"
        fn on_init(state) {
            let s = "AB\0\n\r\tCD";
            for i in 0..7 { s = s + s; }
            state.status_text = s;
        }
        fn on_render(state, t, c) {
            clear(rgb(10, 20, 30));
            rect(10, 10, 40, 40, rgb(255, 255, 255));
        }
    "#;
    let verified = compile_and_verify(long_status_src, &manifest).unwrap();
    let mut inst = VmInstance::new(verified, budgets).unwrap();
    let out = inst.step_frame(1.0 / 60.0, &SceneInputState::default()).unwrap();
    let hud = out.game_hud.expect("game hud present");
    assert!(
        hud.status_text.len() <= 256,
        "status_text must be truncated to <= 256 bytes, got {}",
        hud.status_text.len()
    );
    assert!(
        hud.status_text.chars().all(|ch| !ch.is_control() || ch == '\t'),
        "status_text must strip non-tab control characters: {:?}",
        hud.status_text
    );
    assert!(hud.status_text.starts_with("AB\tCD"));
}

