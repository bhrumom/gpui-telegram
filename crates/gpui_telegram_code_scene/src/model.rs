//! Core leaf domain models, color encoding, input/HUD snapshots, and diagnostics
//! for the pure-Rust `gpui_telegram_code_scene` runtime.

use gpui_telegram_proto::{ValidationDiagnosticDto, ValidationSeverity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Canonical 8-bit RGBA color. Packed into `u32` as `0xRRGGBBAA` for script values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const TRANSPARENT: Self = Self { r: 0, g: 0, b: 0, a: 0 };
    pub const BLACK: Self = Self { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Self = Self { r: 255, g: 255, b: 255, a: 255 };

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn from_u32_rgba(packed: u32) -> Self {
        Self {
            r: ((packed >> 24) & 0xFF) as u8,
            g: ((packed >> 16) & 0xFF) as u8,
            b: ((packed >> 8) & 0xFF) as u8,
            a: (packed & 0xFF) as u8,
        }
    }

    pub const fn to_u32_rgba(self) -> u32 {
        ((self.r as u32) << 24)
            | ((self.g as u32) << 16)
            | ((self.b as u32) << 8)
            | (self.a as u32)
    }

    /// Converts a script numeric color (`0xRRGGBBAA`) safely into `RgbaColor`.
    /// Non-finite values map to `TRANSPARENT` with zero panics.
    pub fn from_script_num(v: f64) -> Self {
        if !v.is_finite() {
            return Self::TRANSPARENT;
        }
        let packed = v.round().clamp(0.0, u32::MAX as f64) as u32;
        Self::from_u32_rgba(packed)
    }

    /// Direct BGRA8 byte ordering `[B, G, R, A]` matching `gpui::RenderImage`.
    pub const fn to_bgra_bytes(self) -> [u8; 4] {
        [self.b, self.g, self.r, self.a]
    }

    pub const fn is_visible(self) -> bool {
        self.a > 0
    }
}

/// Waveform hint for visual audio pulse equalizer bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToneWave {
    #[default]
    Sine,
    Square,
    Triangle,
    Saw,
    Noise,
}

impl ToneWave {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "square" | "sq" => Self::Square,
            "triangle" | "tri" => Self::Triangle,
            "saw" | "sawtooth" => Self::Saw,
            "noise" => Self::Noise,
            _ => Self::Sine,
        }
    }
}

/// Emitted by `tone(freq, dur[, wave[, gain]])` to drive cross-platform audio visualizer pulses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioPulse {
    pub freq_hz: f32,
    pub duration_sec: f32,
    pub wave: ToneWave,
    pub gain: f32,
}

/// Per-frame keyboard and pointer input snapshot passed to `on_update(state, dt, input)`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneInputState {
    pub left: bool,
    pub right: bool,
    pub up: bool,
    pub down: bool,
    pub action: bool,
    pub mouse_x: f64,
    pub mouse_y: f64,
    pub mouse_down: bool,
    pub clicked: bool,
    pub pointer_active: bool,
}

impl SceneInputState {
    pub fn sanitized(&self) -> Self {
        Self {
            left: self.left,
            right: self.right,
            up: self.up,
            down: self.down,
            action: self.action,
            mouse_x: sanitize_f64(self.mouse_x),
            mouse_y: sanitize_f64(self.mouse_y),
            mouse_down: self.mouse_down,
            clicked: self.clicked,
            pointer_active: self.pointer_active,
        }
    }
}

/// Extracted HUD state from playable game `state` map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameHudSnapshot {
    pub score: i64,
    pub high_score: i64,
    pub game_over: bool,
    pub status_text: String,
}

/// Aggregated gameplay engagement metrics for M2 recommender reward shaping.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GameSessionTelemetry {
    pub input_count: u32,
    pub play_ms: u64,
    pub restarts: u32,
    pub best_score: i64,
    pub current_score: i64,
    pub game_over_count: u32,
}

/// Source span (1-indexed line & column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

/// Pipeline stage that produced a compile/validation diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStage {
    Manifest,
    Lex,
    Parse,
    Compile,
    DryRun,
}

impl DiagnosticStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Manifest => "manifest",
            Self::Lex => "lex",
            Self::Parse => "parse",
            Self::Compile => "compile",
            Self::DryRun => "dry_run",
        }
    }
}

/// Structured compile/validation diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[error("[{code}] ({stage}) line {line}, col {col}: {message}", stage = self.stage.as_str(), line = self.line.unwrap_or(0), col = self.col.unwrap_or(0))]
pub struct CompileDiagnostic {
    pub stage: DiagnosticStage,
    pub code: String,
    pub message: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

impl CompileDiagnostic {
    pub fn new(
        stage: DiagnosticStage,
        code: impl Into<String>,
        message: impl Into<String>,
        span: Option<Span>,
    ) -> Self {
        Self {
            stage,
            code: code.into(),
            message: message.into(),
            line: span.map(|s| s.line),
            col: span.map(|s| s.col),
        }
    }

    pub fn to_dto(&self, severity: ValidationSeverity) -> ValidationDiagnosticDto {
        ValidationDiagnosticDto {
            severity,
            stage: Some(self.stage.as_str().to_string()),
            code: self.code.clone(),
            message: self.message.clone(),
            line: self.line,
            column: self.col,
        }
    }
}

/// Runtime errors returned fail-closed by the sandboxed bytecode VM.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeError {
    #[error("instruction budget exceeded ({used} > {limit})")]
    InstructionBudgetExceeded { used: u64, limit: u64 },
    #[error("frame wall-clock timeout exceeded ({elapsed_ms}ms > {limit_ms}ms)")]
    FrameTimeout { elapsed_ms: u64, limit_ms: u64 },
    #[error("call stack overflow (max depth {max_depth})")]
    StackOverflow { max_depth: usize },
    #[error("heap/memory budget exceeded: {detail}")]
    MemoryBudgetExceeded { detail: String },
    #[error("pixel buffer budget exceeded: {detail}")]
    PixelBudgetExceeded { detail: String },
    #[error("video seek replay budget exceeded: {detail}")]
    SeekBudgetExceeded { detail: String },
    #[error("list index out of bounds: {index} (len {len})")]
    IndexOutOfBounds { index: i64, len: usize },
    #[error("type error: {detail}")]
    TypeError { detail: String },
    #[error("undefined function: {name}")]
    UndefinedFunction { name: String },
    #[error("arity mismatch for `{name}`: expected {expected}, got {got}")]
    ArityMismatch {
        name: String,
        expected: String,
        got: usize,
    },
    #[error("invalid operation: {detail}")]
    InvalidOperation { detail: String },
}

pub fn sanitize_f64(v: f64) -> f64 {
    if v.is_finite() { v } else { 0.0 }
}

pub fn canonical_milli(v: f32) -> i32 {
    if !v.is_finite() {
        return 0;
    }
    let normalized = if v == 0.0 { 0.0 } else { v };
    ((normalized as f64) * 1000.0).round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
