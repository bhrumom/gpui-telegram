//! Official 18-asset real-code seed library (`CS-F04`) & collision-free official AI bots.
//!
//! Every asset in `official_seed_assets()` is 100% genuine `CodeSceneScript` (`cs1`)
//! executed exclusively through `compile_and_verify` and `VmInstance`.

use gpui_telegram_proto::{AssetKind, AssetManifest, CODE_SCENE_FORMAT_CS1, CodeSceneAssetDto};

use crate::seeds::{animations_a, animations_b, games_a, games_b, videos_a, videos_b};

pub const OFFICIAL_FABUSHI_BOT_ID: i64 = 777_000;
pub const OFFICIAL_NEON_SHADER_BOT_ID: i64 = 777_101;
pub const OFFICIAL_ARCADE_FORGE_BOT_ID: i64 = 777_102;
pub const OFFICIAL_CINEMA_CODE_BOT_ID: i64 = 777_103;
pub const OFFICIAL_PHYSICS_LAB_BOT_ID: i64 = 777_104;

/// Metadata for an official Code Scene creator bot (safe inside `777_000..778_000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialCodeSceneBot {
    pub user_id: i64,
    pub username: &'static str,
    pub display_name: &'static str,
    pub bio: &'static str,
}

pub const OFFICIAL_CODE_SCENE_BOTS: [OfficialCodeSceneBot; 5] = [
    OfficialCodeSceneBot {
        user_id: OFFICIAL_FABUSHI_BOT_ID,
        username: "fabushi_ai_bot",
        display_name: "Fabushi AI",
        bio: "Official creative AI assistant crafting interactive code videos, shaders, and games.",
    },
    OfficialCodeSceneBot {
        user_id: OFFICIAL_NEON_SHADER_BOT_ID,
        username: "neon_shader_bot",
        display_name: "Neon Shader Lab",
        bio: "Real-time generative pixel shaders, plasma waves, fractals, and neon synth visuals.",
    },
    OfficialCodeSceneBot {
        user_id: OFFICIAL_ARCADE_FORGE_BOT_ID,
        username: "arcade_forge_bot",
        display_name: "Arcade Forge",
        bio: "Playable 60fps retro-futuristic mini-games forged purely in CodeSceneScript.",
    },
    OfficialCodeSceneBot {
        user_id: OFFICIAL_CINEMA_CODE_BOT_ID,
        username: "cinema_code_bot",
        display_name: "Cinema Code",
        bio: "Keyframed motion graphics, kinetic typography, and procedural short films.",
    },
    OfficialCodeSceneBot {
        user_id: OFFICIAL_PHYSICS_LAB_BOT_ID,
        username: "physics_lab_bot",
        display_name: "Physics Lab",
        bio: "Orbital mechanics, swarm simulations, wind-blown fractals, and gravity games.",
    },
];

pub fn find_official_bot(user_id: i64) -> Option<OfficialCodeSceneBot> {
    OFFICIAL_CODE_SCENE_BOTS
        .into_iter()
        .find(|b| b.user_id == user_id)
}

/// Internal specification used by seed submodules to build consistent `CodeSceneAssetDto`s.
pub(crate) struct SeedSpec {
    pub scene_id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub kind: AssetKind,
    pub bot_id: i64,
    pub tags: &'static [&'static str],
    pub duration_sec: Option<f64>,
    pub seed: u64,
    pub controls_hint: &'static str,
    pub plays_count: u64,
    pub likes_count: u64,
    pub bookmarks_count: u64,
    pub comments_count: u64,
    pub shares_count: u64,
    pub remixes_count: u64,
    pub created_at_ms: i64,
    pub source_code: &'static str,
}

pub(crate) fn build_seed_asset(spec: SeedSpec) -> CodeSceneAssetDto {
    let bot = find_official_bot(spec.bot_id).expect("seed bot_id must exist in OFFICIAL_CODE_SCENE_BOTS");
    let manifest = AssetManifest {
        kind: spec.kind,
        width: 360,
        height: 640,
        fps: 60,
        duration_sec: spec.duration_sec,
        seed: spec.seed,
    }
    .clamped();

    CodeSceneAssetDto {
        scene_id: spec.scene_id.to_string(),
        title: spec.title.to_string(),
        description: spec.description.to_string(),
        kind: spec.kind,
        tags: spec.tags.iter().map(|t| (*t).to_string()).collect(),
        author_user_id: bot.user_id,
        author_username: bot.username.to_string(),
        creator_bot_id: Some(bot.user_id),
        creator_bot_username: Some(bot.username.to_string()),
        remix_parent_id: None,
        source_code: spec.source_code.trim().to_string(),
        code_format: CODE_SCENE_FORMAT_CS1.to_string(),
        manifest,
        controls_hint: spec.controls_hint.to_string(),
        plays_count: spec.plays_count,
        likes_count: spec.likes_count,
        bookmarks_count: spec.bookmarks_count,
        comments_count: spec.comments_count,
        shares_count: spec.shares_count,
        remixes_count: spec.remixes_count,
        viewer_liked: false,
        viewer_bookmarked: false,
        viewer_following_author: false,
        created_at_ms: spec.created_at_ms,
        updated_at_ms: spec.created_at_ms,
    }
}

/// Returns all 18 official real-code seed assets (6 Videos, 6 Animations, 6 Playable Games).
pub fn official_seed_assets() -> Vec<CodeSceneAssetDto> {
    let mut out = Vec::with_capacity(18);
    out.extend(videos_a::assets());
    out.extend(videos_b::assets());
    out.extend(animations_a::assets());
    out.extend(animations_b::assets());
    out.extend(games_a::assets());
    out.extend(games_b::assets());
    out
}

/// Looks up an official seed asset by its canonical `scene_id`.
pub fn find_seed_by_id(scene_id: &str) -> Option<CodeSceneAssetDto> {
    official_seed_assets()
        .into_iter()
        .find(|a| a.scene_id == scene_id)
}
