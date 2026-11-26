//! Code Scene ("代码场景") Douyin-style vertical feed workspace for code-authored
//! Videos, Animations, and Playable Interactive Games (`CS-F12`, `CS-F13`, `CS-F14`).

pub mod drawers;
pub mod gesture;
pub mod hud_overlay;
pub mod viewport;
pub mod workspace;

pub use drawers::{ActiveCodeSceneInput, AiStudioState, CodeSceneDrawer, DrawerAction};
pub use gesture::{ease_out_cubic, HeartBurst, SwipeGestureController, SwipeSnapDirection};
pub use hud_overlay::{format_compact_count, format_timecode, HudAction};
pub use viewport::{
    build_render_image_from_pixel_frame, build_vector_path_for_cmd, dedup_request_animation_frame,
    map_point_contain_scene, paint_draw_cmds, CodeSceneViewport, SceneContainRect,
};
pub use workspace::{CodeScenePendingAction, CodeSceneWorkspace, CodeSceneWorkspaceConfig};
