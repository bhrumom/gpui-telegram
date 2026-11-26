//! Runtime execution layer for Code Scene (`Video`, `Animation`, `Game`) + dry-run validator.

pub mod frame;
pub mod pixel;
pub mod session;
pub mod timeline;
pub mod validator;

pub use frame::{DrawCmd, DrawListBuilder, FrameOutput, TextAlign};
pub use pixel::{MAX_PIXEL_AREA, MAX_PIXEL_HEIGHT, MAX_PIXEL_WIDTH, PixelBuffer, PixelFrame};
pub use session::VmInstance;
pub use timeline::{
    MAX_PLAYBACK_SPEED, MAX_SEEK_REPLAY_STEPS, MIN_PLAYBACK_SPEED, SEEK_FIXED_DT,
    TimelineCheckpoint, TimelineController,
};
pub use validator::{
    VerifiedProgram, compile_and_verify, compile_and_verify_with_budgets, compute_cover_hash,
    validate_to_report,
};
