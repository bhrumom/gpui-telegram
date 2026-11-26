//! Pre-publish dry-run validator, canonical cross-CPU SHA-256 `cover_hash`,
//! and `ValidationReportDto` generation (`CS-F03`).

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui_telegram_proto::{
    AssetKind, AssetManifest, ValidationDiagnosticDto, ValidationReportDto, ValidationSeverity,
};
use sha2::{Digest, Sha256};

use crate::lang::{CompiledProgram, VmBudgets, compile_source};
use crate::model::{
    CompileDiagnostic, DiagnosticStage, RgbaColor, SceneInputState, canonical_milli,
};
use crate::runtime::frame::{DrawCmd, FrameOutput};
use crate::runtime::pixel::PixelFrame;
use crate::runtime::session::VmInstance;

/// Compiled and dry-run verified `CodeSceneScript` ready for 60fps execution.
#[derive(Debug, Clone)]
pub struct VerifiedProgram {
    pub compiled: Arc<CompiledProgram>,
    pub manifest: AssetManifest,
    pub cover_hash: String,
    pub feature_tokens: Vec<String>,
    pub peak_ops_used: u64,
    pub draw_cmds_per_frame: Vec<usize>,
    pub has_pixel_output: bool,
    pub warnings: Vec<CompileDiagnostic>,
}

impl VerifiedProgram {
    pub fn has_on_update(&self) -> bool {
        self.compiled.has_function("on_update")
    }
}

/// Compiles `source`, validates lifecycle hooks, dry-runs 5 frames (plus midpoint seek for Video),
/// verifies at least one visible draw command or non-zero-alpha pixel, and computes `cover_hash`.
pub fn compile_and_verify(
    source: &str,
    manifest: &AssetManifest,
) -> Result<VerifiedProgram, CompileDiagnostic> {
    compile_and_verify_with_budgets(source, manifest, VmBudgets::default())
}

pub fn compile_and_verify_with_budgets(
    source: &str,
    manifest: &AssetManifest,
    budgets: VmBudgets,
) -> Result<VerifiedProgram, CompileDiagnostic> {
    let clamped_manifest = manifest.clamped();
    if let Err(msg) = clamped_manifest.validate() {
        return Err(CompileDiagnostic::new(
            DiagnosticStage::Manifest,
            "E_INVALID_MANIFEST",
            msg,
            None,
        ));
    }

    let compiled = Arc::new(compile_source(source, &budgets)?);

    // Validate required/optional lifecycle hooks and parameter bounds
    match compiled.function_arity("on_render") {
        None => {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_MISSING_ON_RENDER",
                "script must define `fn on_render(state, t, canvas)`",
                None,
            ));
        }
        Some(arity) if arity > 3 => {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_BAD_HOOK_ARITY",
                format!("`on_render` accepts at most 3 parameters, got {arity}"),
                None,
            ));
        }
        Some(_) => {}
    }
    if let Some(arity) = compiled.function_arity("on_init") {
        if arity > 1 {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_BAD_HOOK_ARITY",
                format!("`on_init` accepts at most 1 parameter, got {arity}"),
                None,
            ));
        }
    }
    if let Some(arity) = compiled.function_arity("on_update") {
        if arity > 3 {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_BAD_HOOK_ARITY",
                format!("`on_update` accepts at most 3 parameters, got {arity}"),
                None,
            ));
        }
    }

    let mut inst = VmInstance::from_compiled(Arc::clone(&compiled), clamped_manifest.clone(), budgets)
        .map_err(|e| {
            CompileDiagnostic::new(
                DiagnosticStage::DryRun,
                "E_DRY_RUN_INIT",
                format!("on_init failed: {e}"),
                None,
            )
        })?;

    let mut draw_cmds_per_frame = Vec::with_capacity(5);
    let mut peak_ops = 0u64;
    let mut saw_visible = false;
    let mut has_pixel_output = false;
    let mut saw_draw_cap = false;
    let mut last_frame: Option<FrameOutput> = None;

    let neutral = SceneInputState {
        mouse_x: (clamped_manifest.width as f64) * 0.5,
        mouse_y: (clamped_manifest.height as f64) * 0.5,
        ..SceneInputState::default()
    };
    let probe = SceneInputState {
        action: true,
        clicked: true,
        pointer_active: true,
        mouse_x: (clamped_manifest.width as f64) * 0.6,
        mouse_y: (clamped_manifest.height as f64) * 0.4,
        ..SceneInputState::default()
    };

    for frame_idx in 0..5u32 {
        let input = if frame_idx == 3 { &probe } else { &neutral };
        let out = inst.step_frame(1.0 / 60.0, input).map_err(|e| {
            CompileDiagnostic::new(
                DiagnosticStage::DryRun,
                "E_DRY_RUN_FRAME",
                format!("dry-run frame {frame_idx} failed: {e}"),
                None,
            )
        })?;
        peak_ops = peak_ops.max(out.ops_used);
        draw_cmds_per_frame.push(out.draw_cmds.len());
        if inst.last_draw_cap_hit() {
            saw_draw_cap = true;
        }
        if out.draw_cmds.iter().any(DrawCmd::is_visible) {
            saw_visible = true;
        }
        if let Some(ref pf) = out.pixel_bgra {
            if pf.has_visible_pixel() {
                saw_visible = true;
                has_pixel_output = true;
            }
        }
        last_frame = Some(out);
    }

    let cover_frame = last_frame.expect("5 frames executed");
    let cover_pixel = cover_frame
        .pixel_bgra
        .clone()
        .or_else(|| inst.pixel_snapshot());

    // For videos, also probe midpoint seek determinism/safety
    if clamped_manifest.kind == AssetKind::Video {
        if let Some(dur) = clamped_manifest.duration_sec {
            let seek_out = inst.seek_video(dur * 0.5).map_err(|e| {
                CompileDiagnostic::new(
                    DiagnosticStage::DryRun,
                    "E_DRY_RUN_SEEK",
                    format!("video midpoint seek failed: {e}"),
                    None,
                )
            })?;
            if seek_out.draw_cmds.iter().any(DrawCmd::is_visible) {
                saw_visible = true;
            }
            if let Some(ref pf) = seek_out.pixel_bgra {
                if pf.has_visible_pixel() {
                    saw_visible = true;
                    has_pixel_output = true;
                }
            }
        }
    }

    if !saw_visible {
        return Err(CompileDiagnostic::new(
            DiagnosticStage::DryRun,
            "E_NO_VISIBLE_OUTPUT",
            "script produced zero visible draw commands or non-zero-alpha pixels across 5 frames",
            None,
        ));
    }

    let cover_hash = compute_cover_hash(
        clamped_manifest.width,
        clamped_manifest.height,
        &cover_frame.draw_cmds,
        cover_pixel.as_ref(),
    );

    let mut token_set: BTreeSet<String> = compiled.feature_tokens.iter().cloned().collect();
    for dyn_tok in inst.used_builtins() {
        token_set.insert(dyn_tok);
    }

    let mut warnings = Vec::new();
    if saw_draw_cap {
        warnings.push(CompileDiagnostic::new(
            DiagnosticStage::DryRun,
            "W_DRAW_CAP_HIT",
            format!("draw commands exceeded cap ({}) and were truncated", budgets.max_draw_cmds),
            None,
        ));
    }

    Ok(VerifiedProgram {
        compiled,
        manifest: clamped_manifest,
        cover_hash,
        feature_tokens: token_set.into_iter().collect(),
        peak_ops_used: peak_ops,
        draw_cmds_per_frame,
        has_pixel_output,
        warnings,
    })
}

/// Runs full compile + 5-frame dry-run validation and maps the outcome into `ValidationReportDto`.
pub fn validate_to_report(source: &str, manifest: &AssetManifest) -> ValidationReportDto {
    match compile_and_verify(source, manifest) {
        Ok(verified) => {
            let diagnostics: Vec<ValidationDiagnosticDto> = verified
                .warnings
                .iter()
                .map(|w| w.to_dto(ValidationSeverity::Warning))
                .collect();
            ValidationReportDto {
                ok: true,
                diagnostics,
                frames_executed: verified.draw_cmds_per_frame.len() as u32,
                draw_cmds_per_frame: verified.draw_cmds_per_frame,
                has_pixel_output: verified.has_pixel_output,
                peak_ops_used: verified.peak_ops_used,
                cover_hash: Some(verified.cover_hash),
                feature_tokens: verified.feature_tokens,
            }
        }
        Err(err) => ValidationReportDto {
            ok: false,
            diagnostics: vec![err.to_dto(ValidationSeverity::Error)],
            frames_executed: 0,
            draw_cmds_per_frame: Vec::new(),
            has_pixel_output: false,
            peak_ops_used: 0,
            cover_hash: None,
            feature_tokens: Vec::new(),
        },
    }
}

/// Computes a deterministic, cross-CPU, wall-clock-independent 64-char SHA-256 hex digest
/// over quantized milli-pixel `DrawCmd`s and optional BGRA8 `PixelFrame`.
pub fn compute_cover_hash(
    width: u32,
    height: u32,
    draw_cmds: &[DrawCmd],
    pixel: Option<&PixelFrame>,
) -> String {
    let mut h = Sha256::new();
    h.update(b"cs1-cover-v1\0");
    h.update(width.to_le_bytes());
    h.update(height.to_le_bytes());
    h.update((draw_cmds.len() as u32).to_le_bytes());
    for cmd in draw_cmds {
        hash_draw_cmd(&mut h, cmd);
    }
    if let Some(pf) = pixel {
        h.update(b"PX\0");
        h.update(pf.width.to_le_bytes());
        h.update(pf.height.to_le_bytes());
        h.update(&pf.bgra);
    }
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

fn hash_rgba(h: &mut Sha256, c: RgbaColor) {
    h.update([c.r, c.g, c.b, c.a]);
}

fn hash_f32(h: &mut Sha256, v: f32) {
    h.update(canonical_milli(v).to_le_bytes());
}

fn hash_draw_cmd(h: &mut Sha256, cmd: &DrawCmd) {
    match cmd {
        DrawCmd::Clear { color } => {
            h.update([1]);
            hash_rgba(h, *color);
        }
        DrawCmd::LinearGradientBg { top, bottom, angle_deg } => {
            h.update([2]);
            hash_rgba(h, *top);
            hash_rgba(h, *bottom);
            hash_f32(h, *angle_deg);
        }
        DrawCmd::FillRect { x, y, w, h: rh, color } => {
            h.update([3]);
            for v in [*x, *y, *w, *rh] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::RoundRect { x, y, w, h: rh, radius, color } => {
            h.update([4]);
            for v in [*x, *y, *w, *rh, *radius] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::StrokeRect { x, y, w, h: rh, stroke_width, color } => {
            h.update([5]);
            for v in [*x, *y, *w, *rh, *stroke_width] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::FillCircle { cx, cy, radius, color } => {
            h.update([6]);
            for v in [*cx, *cy, *radius] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::StrokeCircle { cx, cy, radius, stroke_width, color } => {
            h.update([7]);
            for v in [*cx, *cy, *radius, *stroke_width] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::GlowCircle { cx, cy, radius, color, intensity } => {
            h.update([8]);
            for v in [*cx, *cy, *radius, *intensity] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::Line { x1, y1, x2, y2, stroke_width, color } => {
            h.update([9]);
            for v in [*x1, *y1, *x2, *y2, *stroke_width] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::Polyline { points, stroke_width, color } => {
            h.update([10]);
            h.update((points.len() as u32).to_le_bytes());
            for p in points { hash_f32(h, p[0]); hash_f32(h, p[1]); }
            hash_f32(h, *stroke_width);
            hash_rgba(h, *color);
        }
        DrawCmd::FillPolygon { points, color } => {
            h.update([11]);
            h.update((points.len() as u32).to_le_bytes());
            for p in points { hash_f32(h, p[0]); hash_f32(h, p[1]); }
            hash_rgba(h, *color);
        }
        DrawCmd::Arc { cx, cy, radius, start_rad, end_rad, stroke_width, color } => {
            h.update([12]);
            for v in [*cx, *cy, *radius, *start_rad, *end_rad, *stroke_width] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::Bezier { x0, y0, cx1, cy1, cx2, cy2, x1, y1, stroke_width, color } => {
            h.update([13]);
            for v in [*x0, *y0, *cx1, *cy1, *cx2, *cy2, *x1, *y1, *stroke_width] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::Star { cx, cy, outer_r, inner_r, points, rotation_rad, color } => {
            h.update([14, *points]);
            for v in [*cx, *cy, *outer_r, *inner_r, *rotation_rad] { hash_f32(h, v); }
            hash_rgba(h, *color);
        }
        DrawCmd::Text { x, y, content, size, color, align } => {
            h.update([15, *align as u8]);
            for v in [*x, *y, *size] { hash_f32(h, v); }
            h.update((content.len() as u32).to_le_bytes());
            h.update(content.as_bytes());
            hash_rgba(h, *color);
        }
    }
}
