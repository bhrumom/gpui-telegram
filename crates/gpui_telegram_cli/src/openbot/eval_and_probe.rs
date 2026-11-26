//! `openbot avatar-eval`, `openbot theme-eval`, and `openbot ui-probe` handlers (`OB-F01`..`OB-F20`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use gpui_telegram_desktop::components::bloub_avatar::{
    AvatarEvalError, AvatarEvalRequest, evaluate_avatar_spec, parse_pointer_kind,
    parse_pointer_pair,
};
use gpui_telegram_desktop::theme::openbot_palette::{
    ThemeEvalRequest, ThemePaletteError, evaluate_theme_spec,
};
use gpui_telegram_desktop::views::openbot::ui_probe::{
    LiveFusionSnapshot, UiProbeError, evaluate_ui_probe,
};
use openbot_contracts::channels::SIGNED_OUT_CHANNEL_MEMBER_ID;
use openbot_storage::OpenBotDatabase;
use serde_json::Value;
use uuid::Uuid;

use super::{
    FlagSpec, ParsedFlags, map_storage_error, parse_f64_domain, parse_u16_domain, parse_u32_domain,
    parse_usize_domain,
};
use crate::error::CliError;

pub fn avatar_eval_spec() -> FlagSpec {
    FlagSpec {
        value_flags: &[
            "--seed",
            "--hue",
            "--color",
            "--style",
            "--shape",
            "--mood",
            "--state",
            "--expression",
            "--t",
            "--pointer",
            "--pointer-type",
            "--cluster-count",
            "--normalize-image",
        ],
        bool_flags: &["--reduced-motion", "--working-faces"],
        opt_bool_flags: &[],
        required_flags: &[],
    }
}

pub fn theme_eval_spec() -> FlagSpec {
    FlagSpec {
        value_flags: &[
            "--mode",
            "--theme-json",
            "--themes-dir",
            "--backdrop",
            "--chat-color",
            "--viewport-width",
            "--viewport-height",
            "--elapsed-sec",
        ],
        bool_flags: &["--reduced-motion", "--persist"],
        opt_bool_flags: &[],
        required_flags: &[],
    }
}

pub fn ui_probe_spec() -> FlagSpec {
    FlagSpec {
        value_flags: &["--scenario"],
        bool_flags: &[],
        opt_bool_flags: &[],
        required_flags: &["--scenario"],
    }
}

fn map_avatar_err(err: AvatarEvalError) -> CliError {
    if err.is_validation_error() {
        CliError::Validation(err.to_string())
    } else {
        CliError::Execution(err.to_string())
    }
}

fn map_theme_err(err: ThemePaletteError) -> CliError {
    match err {
        ThemePaletteError::Io(io_err) => CliError::Execution(io_err.to_string()),
        other => CliError::Validation(other.to_string()),
    }
}

pub fn run_avatar_eval(flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let hue = match flags.get("--hue") {
        Some(raw) => Some(parse_u16_domain(raw, "avatar hue")?),
        None => None,
    };
    let t = match flags.get("--t") {
        Some(raw) => Some(parse_f64_domain(raw, "time t")?),
        None => None,
    };
    let pointer = match flags.get("--pointer") {
        Some(raw) => Some(parse_pointer_pair(raw).map_err(map_avatar_err)?),
        None => None,
    };
    let pointer_type = match flags.get("--pointer-type") {
        Some(raw) => {
            let _ = parse_pointer_kind(raw).map_err(map_avatar_err)?;
            Some(raw.to_string())
        }
        None => None,
    };
    let cluster_count = match flags.get("--cluster-count") {
        Some(raw) => Some(parse_usize_domain(raw, "cluster count")?),
        None => None,
    };
    let (normalize_image, normalize_output_path) = match flags.get("--normalize-image") {
        Some(raw) => {
            let out_dir = data_dir.join("normalized-avatars");
            let _ = fs::create_dir_all(&out_dir);
            let out_path = out_dir.join(format!("avatar-{}.webp", Uuid::new_v4()));
            (Some(PathBuf::from(raw)), Some(out_path))
        }
        None => (None, None),
    };

    let req = AvatarEvalRequest {
        seed: flags.get("--seed").unwrap_or("").to_string(),
        hue,
        color: flags.get("--color").map(str::to_string),
        style: flags.get("--style").map(str::to_string),
        shape: flags.get("--shape").map(str::to_string),
        mood: flags.get("--mood").map(str::to_string),
        state: flags.get("--state").map(str::to_string),
        expression: flags.get("--expression").map(str::to_string),
        t,
        pointer,
        pointer_type,
        reduced_motion: flags.has("--reduced-motion"),
        working_faces: flags.has("--working-faces"),
        cluster_count,
        normalize_image,
        normalize_output_path,
    };

    let report = evaluate_avatar_spec(&req).map_err(map_avatar_err)?;
    serde_json::to_value(report)
        .map_err(|e| CliError::Execution(format!("failed to serialize avatar report: {e}")))
}

pub fn run_theme_eval(flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let viewport_width_px = match flags.get("--viewport-width") {
        Some(raw) => Some(parse_u32_domain(raw, "viewport width")?),
        None => None,
    };
    let viewport_height_px = match flags.get("--viewport-height") {
        Some(raw) => Some(parse_u32_domain(raw, "viewport height")?),
        None => None,
    };
    let elapsed_sec = match flags.get("--elapsed-sec") {
        Some(raw) => Some(parse_f64_domain(raw, "elapsed seconds")?),
        None => None,
    };

    let req = ThemeEvalRequest {
        mode: flags.get("--mode").map(str::to_string),
        theme_json: flags.get("--theme-json").map(PathBuf::from),
        themes_dir: flags.get("--themes-dir").map(PathBuf::from),
        data_dir: Some(data_dir.to_path_buf()),
        backdrop: flags.get("--backdrop").map(str::to_string),
        chat_color: flags.get("--chat-color").map(str::to_string),
        reduced_motion: flags.has("--reduced-motion"),
        persist: flags.has("--persist"),
        viewport_width_px,
        viewport_height_px,
        elapsed_sec,
    };

    let report = evaluate_theme_spec(&req).map_err(map_theme_err)?;
    serde_json::to_value(report)
        .map_err(|e| CliError::Execution(format!("failed to serialize theme report: {e}")))
}

fn load_live_fusion_snapshot(data_dir: &Path) -> Result<LiveFusionSnapshot, CliError> {
    let db_path = data_dir.join("openbot.db");
    if !db_path.is_file() {
        return Ok(LiveFusionSnapshot::default());
    }
    let db = OpenBotDatabase::open(&db_path).map_err(map_storage_error)?;
    let agents = db.agent_roster().list_agents().map_err(map_storage_error)?;
    let channels = db
        .channel_store()
        .list(SIGNED_OUT_CHANNEL_MEMBER_ID, false)
        .map_err(map_storage_error)?;

    let mention_candidates: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
    let projected_channel_peers = channels.iter().filter(|c| !c.channel.archived).count();
    let mut channel_members_by_id = HashMap::with_capacity(channels.len());
    for entry in &channels {
        let members: Vec<String> = entry
            .channel
            .draft
            .members
            .iter()
            .map(|m| m.agent_id.clone())
            .collect();
        channel_members_by_id.insert(entry.channel.id.clone(), members);
    }

    Ok(LiveFusionSnapshot {
        projected_agent_peers: agents.len(),
        projected_channel_peers,
        mention_candidates,
        channel_members_by_id,
    })
}

pub fn run_ui_probe(flags: &ParsedFlags, data_dir: &Path) -> Result<Value, CliError> {
    let scenario = flags.require("--scenario")?;
    let snapshot = load_live_fusion_snapshot(data_dir)?;
    evaluate_ui_probe(scenario, data_dir, Some(&snapshot)).map_err(|err| match err {
        UiProbeError::Validation(m) => CliError::Validation(m),
        UiProbeError::Forbidden(m) => CliError::Forbidden(m),
        UiProbeError::Execution(m) => CliError::Execution(m),
    })
}
