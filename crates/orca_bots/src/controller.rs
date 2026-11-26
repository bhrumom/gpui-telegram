//! Serialized async `BotController` implementing 22-agent CRUD, rollback-safe split
//! persistence (`bots.json` + `bots_newer.json`), functional read-modify-write patches,
//! host-only MCP toggle, and CAS-guarded avatar mutations.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use orca_core::bot_types::{
    AcceptedBotOffer, BotAgentId, BotAvatarSpec, BotConnectorLinks, BotGitHubLink,
    BotImportedFrom, BotItemsStale, BotLaunchConfig, BotLinearLink, BotLookAroundConfig,
    BotMascotId, BotMember, BotName, BotNotifications, BotOfferAnswerUpdate, BotPermissionMode,
    BotSharedFrom, BotSlackLink, BotTidyUpConfig, OrcaBotRecord, apply_bot_offer_answer,
    bot_member_source, new_bot_name, normalize_persisted_bots, split_bots_by_older_build_support,
    without_bot_member,
};
use orca_core::{BotErrorCode, LOCAL_EXECUTION_HOST_ID};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::home_store::{BotHomeStore, WorktreeReleaseBotHome, atomic_write_bytes};
use crate::{OrcaBotsError, Result, now_epoch_ms};

const BOTS_LEGACY_FILE: &str = "bots.json";
const BOTS_NEWER_FILE: &str = "bots_newer.json";

/// Input payload for creating a new bot.
#[derive(Debug, Clone, Default)]
pub struct CreateBotRequest {
    pub name: Option<String>,
    pub agent: BotAgentId,
    pub mascot: Option<BotMascotId>,
    pub avatar_color: Option<String>,
    pub avatar_outfit: Option<String>,
    pub custom_soul: Option<String>,
    pub permission_mode: Option<BotPermissionMode>,
    pub launch: Option<BotLaunchConfig>,
    pub folder_workspace_id: Option<String>,
    pub template_id: Option<String>,
    pub first_run: bool,
    pub imported_from: Option<BotImportedFrom>,
    pub shared_from: Option<BotSharedFrom>,
}

/// Identifies a single member entry to remove from a bot's member list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotMemberRemoval {
    pub source: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// General connector patch fields. Deliberately omits `host_mcp_servers`, which may ONLY
/// be toggled via the dedicated host-only `BotController::set_host_mcp_servers` method.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotConnectorPatch {
    pub slack: Option<Option<BotSlackLink>>,
    pub slack_apps: Option<Option<Vec<BotSlackLink>>>,
    pub linear: Option<Option<BotLinearLink>>,
    pub linear_workspaces: Option<Option<Vec<BotLinearLink>>>,
    pub github: Option<Option<BotGitHubLink>>,
    pub slack_mirror: Option<Option<bool>>,
    pub google: Option<Option<bool>>,
    pub google_accounts: Option<Option<Vec<String>>>,
}

/// Functional read-modify-write patch for `BotController::update_bot`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BotPatch {
    pub name: Option<String>,
    pub agent: Option<BotAgentId>,
    pub paused: Option<bool>,
    pub permission_mode: Option<Option<BotPermissionMode>>,
    pub launch: Option<Option<BotLaunchConfig>>,
    pub notifications: Option<BotNotifications>,
    pub add_members: Vec<BotMember>,
    pub remove_member: Option<BotMemberRemoval>,
    pub remove_members: Vec<BotMemberRemoval>,
    pub connectors: Option<BotConnectorPatch>,
    pub skill_projects: Option<Option<Vec<String>>>,
    pub sidebar_order: Option<Option<u32>>,
    pub chat_session_id: Option<Option<String>>,
    pub look_around: Option<Option<BotLookAroundConfig>>,
    pub tidy_up: Option<Option<BotTidyUpConfig>>,
    pub items_stale: Option<Option<BotItemsStale>>,
    pub items_seen: Option<Option<u64>>,
    pub first_run: Option<bool>,
    pub offer_answer: Option<BotOfferAnswerUpdate>,
    pub accept_offer: Option<AcceptedBotOffer>,
    pub folder_workspace_id: Option<Option<String>>,
}

/// Request to update a bot's mascot/emoji, color, and/or outfit with optional CAS guard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateAvatarStyleRequest {
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub outfit: Option<String>,
    pub expected_avatar_revision: Option<u64>,
    pub expected_avatar_revision_epoch: Option<String>,
}

impl UpdateAvatarStyleRequest {
    /// Convenience constructor selecting a built-in mascot (`A1..T1`), color, and outfit.
    pub fn mascot(
        mascot: BotMascotId,
        color: Option<String>,
        outfit: Option<String>,
        expected_avatar_revision: Option<u64>,
        expected_avatar_revision_epoch: Option<String>,
    ) -> Self {
        Self {
            avatar: Some(format!("mascot:{}", mascot.as_str())),
            color,
            outfit,
            expected_avatar_revision,
            expected_avatar_revision_epoch,
        }
    }
}

/// Request to upload a validated square PNG avatar with optional CAS guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadAvatarPngRequest {
    pub png_bytes: Vec<u8>,
    pub expected_avatar_revision: Option<u64>,
    pub expected_avatar_revision_epoch: Option<String>,
}

#[derive(Debug)]
struct BotControllerState {
    bots: Vec<OrcaBotRecord>,
}

/// Serialized host controller managing bot records and home directories.
#[derive(Debug, Clone)]
pub struct BotController {
    state_dir: PathBuf,
    home_store: BotHomeStore,
    inner: Arc<Mutex<BotControllerState>>,
}

impl BotController {
    /// Opens `BotController` backed by `state_dir` (`bots.json` + `bots_newer.json`)
    /// and `home_store`, normalizing persisted records and migrating retired agents.
    pub fn open(state_dir: impl Into<PathBuf>, home_store: BotHomeStore) -> Result<Self> {
        let state_dir = state_dir.into();
        fs::create_dir_all(&state_dir)?;
        let bots = load_split_bots(&state_dir)?;
        Ok(Self {
            state_dir,
            home_store,
            inner: Arc::new(Mutex::new(BotControllerState { bots })),
        })
    }

    /// Returns the state directory where `bots.json` and `bots_newer.json` are persisted.
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Returns a reference to the underlying `BotHomeStore`.
    pub fn home_store(&self) -> &BotHomeStore {
        &self.home_store
    }

    /// Lists all bots currently registered on this host.
    pub async fn list_bots(&self) -> Vec<OrcaBotRecord> {
        let guard = self.inner.lock().await;
        guard.bots.clone()
    }

    /// Fetches a single bot by `bot_id`, failing with `BotErrorCode::BotNotFound` if absent.
    pub async fn get_bot(&self, bot_id: &str) -> Result<OrcaBotRecord> {
        let guard = self.inner.lock().await;
        guard
            .bots
            .iter()
            .find(|b| b.id == bot_id)
            .cloned()
            .ok_or_else(|| {
                OrcaBotsError::bot(BotErrorCode::BotNotFound, format!("Bot '{bot_id}' not found."))
            })
    }

    /// Creates a new bot: validates or auto-generates a unique name (`New bot`, `New bot 2`, ...),
    /// atomically reserves its folder, provisions its home files, and persists split storage.
    pub async fn create_bot(&self, req: CreateBotRequest) -> Result<OrcaBotRecord> {
        let mut guard = self.inner.lock().await;
        let existing_names: Vec<&str> = guard.bots.iter().map(|b| b.name.as_str()).collect();
        let requested = req.name.as_deref().unwrap_or("");
        let name = new_bot_name(requested, &existing_names)?;
        if guard
            .bots
            .iter()
            .any(|b| b.name.as_str().eq_ignore_ascii_case(name.as_str()))
        {
            return Err(OrcaBotsError::bot(
                BotErrorCode::BotNameTaken,
                format!("Bot name '{}' is already taken.", name.as_str()),
            ));
        }

        let mascot = req.mascot.unwrap_or(BotMascotId::A1);
        let mut avatar_spec = BotAvatarSpec::new_with_mascot(mascot);
        if req.avatar_color.is_some() || req.avatar_outfit.is_some() {
            let color = req.avatar_color.or_else(|| avatar_spec.avatar_color.clone());
            avatar_spec.apply_manual_look_update(
                avatar_spec.avatar.clone(),
                color,
                req.avatar_outfit,
            )?;
            avatar_spec.avatar_revision = 0;
        }

        let (folder, reserved_dir) = self.home_store.reserve_unique_folder(name.as_str())?;
        let now = now_epoch_ms();
        let record = OrcaBotRecord {
            id: Uuid::new_v4().to_string(),
            name,
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder,
            folder_workspace_id: req.folder_workspace_id,
            agent: req.agent,
            avatar_spec,
            paused: false,
            permission_mode: req.permission_mode,
            launch: req.launch,
            notifications: BotNotifications::default(),
            members: Vec::new(),
            connectors: BotConnectorLinks::default(),
            skill_projects: None,
            sidebar_order: None,
            chat_session_id: None,
            look_around: None,
            tidy_up: None,
            items_stale: None,
            items_seen: None,
            first_run: req.first_run,
            accepted_offers: None,
            offer_answers: None,
            handed_paired_workspaces: None,
            cancelled_waits: None,
            imported_from: req.imported_from,
            shared_from: req.shared_from,
            template_id: req.template_id,
            created_at: now,
            updated_at: now,
        };

        if let Err(err) = record.validate() {
            let _ = fs::remove_dir_all(&reserved_dir);
            return Err(OrcaBotsError::from(err));
        }

        if let Err(err) = self
            .home_store
            .provision_bot_home(&record, req.custom_soul.as_deref())
        {
            let _ = fs::remove_dir_all(&reserved_dir);
            return Err(err);
        }

        guard.bots.push(record.clone());
        if let Err(err) = save_split_bots(&self.state_dir, &guard.bots) {
            guard.bots.pop();
            let _ = fs::remove_dir_all(&reserved_dir);
            return Err(err);
        }
        Ok(record)
    }

    /// Applies a functional read-modify-write `BotPatch` inside the serialized mutation queue.
    /// General patches NEVER modify `connectors.host_mcp_servers`.
    pub async fn update_bot(&self, bot_id: &str, patch: BotPatch) -> Result<OrcaBotRecord> {
        let mut guard = self.inner.lock().await;
        let idx = find_bot_index(&guard.bots, bot_id)?;
        let original_host_mcp = guard.bots[idx].connectors.host_mcp_servers;
        let mut candidate = guard.bots[idx].clone();

        if let Some(raw_name) = patch.name {
            let next_name = BotName::new(&raw_name)?;
            let taken = guard.bots.iter().enumerate().any(|(i, other)| {
                i != idx && other.name.as_str().eq_ignore_ascii_case(next_name.as_str())
            });
            if taken {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::BotNameTaken,
                    format!("Bot name '{}' is already taken.", next_name.as_str()),
                ));
            }
            candidate.name = next_name;
        }
        if let Some(agent) = patch.agent {
            candidate.agent = agent;
        }
        if let Some(paused) = patch.paused {
            candidate.paused = paused;
        }
        if let Some(mode) = patch.permission_mode {
            candidate.permission_mode = mode;
        }
        if let Some(launch) = patch.launch {
            candidate.launch = launch;
        }
        if let Some(notifications) = patch.notifications {
            candidate.notifications = notifications;
        }
        apply_member_mutations(
            &mut candidate.members,
            patch.add_members,
            patch.remove_member,
            patch.remove_members,
        );
        if let Some(conn_patch) = patch.connectors {
            apply_connector_patch(&mut candidate.connectors, conn_patch);
        }
        if let Some(projects) = patch.skill_projects {
            candidate.skill_projects = projects;
        }
        if let Some(order) = patch.sidebar_order {
            candidate.sidebar_order = order;
        }
        if let Some(chat_sid) = patch.chat_session_id {
            candidate.chat_session_id = chat_sid;
        }
        if let Some(la) = patch.look_around {
            candidate.look_around = la;
        }
        if let Some(tu) = patch.tidy_up {
            candidate.tidy_up = tu;
        }
        if let Some(stale) = patch.items_stale {
            candidate.items_stale = stale;
        }
        if let Some(seen) = patch.items_seen {
            candidate.items_seen = seen;
        }
        if let Some(first_run) = patch.first_run {
            candidate.first_run = first_run;
        }
        if let Some(offer_update) = patch.offer_answer {
            let existing = candidate.offer_answers.as_deref().unwrap_or(&[]);
            let next_answers = apply_bot_offer_answer(existing, offer_update)?;
            candidate.offer_answers = Some(next_answers);
        }
        if let Some(accepted) = patch.accept_offer {
            let offers = candidate.accepted_offers.get_or_insert_with(Vec::new);
            if !offers.iter().any(|o| o.offer_id == accepted.offer_id) {
                offers.push(accepted);
            }
        }
        if let Some(ws_id) = patch.folder_workspace_id {
            candidate.folder_workspace_id = ws_id;
        }

        // Enforce host-only invariant: general patches can never mutate `host_mcp_servers`.
        candidate.connectors.host_mcp_servers = original_host_mcp;
        candidate.validate()?;
        candidate.updated_at = now_epoch_ms();

        self.home_store.sync_generated_home_files(&candidate)?;
        let prev = std::mem::replace(&mut guard.bots[idx], candidate.clone());
        if let Err(err) = save_split_bots(&self.state_dir, &guard.bots) {
            guard.bots[idx] = prev;
            return Err(err);
        }
        Ok(candidate)
    }

    /// Dedicated host-only toggle for `connectors.host_mcp_servers`.
    pub async fn set_host_mcp_servers(&self, bot_id: &str, enabled: bool) -> Result<OrcaBotRecord> {
        let mut guard = self.inner.lock().await;
        let idx = find_bot_index(&guard.bots, bot_id)?;
        let mut candidate = guard.bots[idx].clone();
        candidate.connectors.host_mcp_servers = Some(enabled);
        candidate.updated_at = now_epoch_ms();

        self.home_store.sync_generated_home_files(&candidate)?;
        let prev = std::mem::replace(&mut guard.bots[idx], candidate.clone());
        if let Err(err) = save_split_bots(&self.state_dir, &guard.bots) {
            guard.bots[idx] = prev;
            return Err(err);
        }
        Ok(candidate)
    }

    /// Updates a bot's mascot/emoji, color, and/or outfit with CAS revision verification.
    pub async fn update_avatar_style(
        &self,
        bot_id: &str,
        req: UpdateAvatarStyleRequest,
    ) -> Result<OrcaBotRecord> {
        let mut guard = self.inner.lock().await;
        let idx = find_bot_index(&guard.bots, bot_id)?;
        let mut candidate = guard.bots[idx].clone();

        if let Some(expected_rev) = req.expected_avatar_revision {
            candidate
                .avatar_spec
                .check_cas(expected_rev, req.expected_avatar_revision_epoch.as_deref())?;
        }
        candidate
            .avatar_spec
            .apply_manual_look_update(req.avatar, req.color, req.outfit)?;
        candidate.updated_at = now_epoch_ms();

        self.home_store.sync_generated_home_files(&candidate)?;
        let prev = std::mem::replace(&mut guard.bots[idx], candidate.clone());
        if let Err(err) = save_split_bots(&self.state_dir, &guard.bots) {
            guard.bots[idx] = prev;
            return Err(err);
        }
        Ok(candidate)
    }

    /// Validates PNG bytes, checks CAS avatar revision/epoch, writes `avatar.png` atomically,
    /// bumps `avatar_revision`, syncs `bot.json`, and persists split storage.
    pub async fn upload_avatar_png(
        &self,
        bot_id: &str,
        req: UploadAvatarPngRequest,
    ) -> Result<OrcaBotRecord> {
        let mut guard = self.inner.lock().await;
        let idx = find_bot_index(&guard.bots, bot_id)?;
        let mut candidate = guard.bots[idx].clone();

        candidate.avatar_spec.apply_custom_png_upload(
            &req.png_bytes,
            req.expected_avatar_revision,
            req.expected_avatar_revision_epoch.as_deref(),
        )?;
        candidate.updated_at = now_epoch_ms();

        self.home_store
            .write_avatar_png(&candidate.folder, &candidate.id, &req.png_bytes)?;
        self.home_store.sync_generated_home_files(&candidate)?;

        let prev = std::mem::replace(&mut guard.bots[idx], candidate.clone());
        if let Err(err) = save_split_bots(&self.state_dir, &guard.bots) {
            guard.bots[idx] = prev;
            return Err(err);
        }
        Ok(candidate)
    }

    /// Deletes a bot after strictly verifying `bot.json` home ownership and removing its folder.
    pub async fn delete_bot(&self, bot_id: &str) -> Result<WorktreeReleaseBotHome> {
        let mut guard = self.inner.lock().await;
        let idx = find_bot_index(&guard.bots, bot_id)?;
        let bot = guard.bots[idx].clone();

        let release = self.home_store.release_bot_home(
            &bot.folder,
            &bot.id,
            bot.folder_workspace_id.clone(),
        )?;
        guard.bots.remove(idx);
        save_split_bots(&self.state_dir, &guard.bots)?;
        Ok(release)
    }
}

fn find_bot_index(bots: &[OrcaBotRecord], bot_id: &str) -> Result<usize> {
    bots.iter().position(|b| b.id == bot_id).ok_or_else(|| {
        OrcaBotsError::bot(BotErrorCode::BotNotFound, format!("Bot '{bot_id}' not found."))
    })
}

fn apply_member_mutations(
    members: &mut Vec<BotMember>,
    add_members: Vec<BotMember>,
    remove_member: Option<BotMemberRemoval>,
    remove_members: Vec<BotMemberRemoval>,
) {
    let mut seen: HashSet<(String, String, Option<String>)> = members
        .iter()
        .map(|m| {
            (
                bot_member_source(m).to_string(),
                m.id.clone(),
                m.workspace_id.clone(),
            )
        })
        .collect();
    for m in add_members {
        let key = (
            bot_member_source(&m).to_string(),
            m.id.clone(),
            m.workspace_id.clone(),
        );
        if seen.insert(key) {
            members.push(m);
        }
    }

    for removal in remove_member.into_iter().chain(remove_members) {
        *members = without_bot_member(
            members,
            &removal.source,
            &removal.id,
            removal.workspace_id.as_deref(),
        );
    }
}

fn apply_connector_patch(target: &mut BotConnectorLinks, patch: BotConnectorPatch) {
    if let Some(v) = patch.slack {
        target.slack = v;
    }
    if let Some(v) = patch.slack_apps {
        target.slack_apps = v;
    }
    if let Some(v) = patch.linear {
        target.linear = v;
    }
    if let Some(v) = patch.linear_workspaces {
        target.linear_workspaces = v;
    }
    if let Some(v) = patch.github {
        target.github = v;
    }
    if let Some(v) = patch.slack_mirror {
        target.slack_mirror = v;
    }
    if let Some(v) = patch.google {
        target.google = v;
    }
    if let Some(v) = patch.google_accounts {
        target.google_accounts = v;
    }
}

fn read_raw_json_array(path: &Path) -> Result<Vec<serde_json::Value>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(Vec::new());
            }
            let parsed: serde_json::Value = serde_json::from_str(trimmed)?;
            Ok(parsed.as_array().cloned().unwrap_or_default())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(OrcaBotsError::Io(err)),
    }
}

fn load_split_bots(state_dir: &Path) -> Result<Vec<OrcaBotRecord>> {
    let mut raw_values = read_raw_json_array(&state_dir.join(BOTS_LEGACY_FILE))?;
    raw_values.extend(read_raw_json_array(&state_dir.join(BOTS_NEWER_FILE))?);
    Ok(normalize_persisted_bots(&raw_values))
}

fn save_split_bots(state_dir: &Path, bots: &[OrcaBotRecord]) -> Result<()> {
    let split = split_bots_by_older_build_support(bots);
    let mut legacy_json = serde_json::to_string_pretty(&split.readable)?;
    legacy_json.push('\n');
    let mut newer_json = serde_json::to_string_pretty(&split.newer)?;
    newer_json.push('\n');

    atomic_write_bytes(&state_dir.join(BOTS_LEGACY_FILE), legacy_json.as_bytes())?;
    atomic_write_bytes(&state_dir.join(BOTS_NEWER_FILE), newer_json.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::bot_types::{
        BotOfferAnswerState, BotOfferAnswerUpdate, BOT_AGENTS_OLDER_BUILDS_READ,
    };
    use tempfile::TempDir;

    fn make_valid_png(width: u32, height: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        buf.extend_from_slice(&13u32.to_be_bytes());
        buf.extend_from_slice(b"IHDR");
        buf.extend_from_slice(&width.to_be_bytes());
        buf.extend_from_slice(&height.to_be_bytes());
        buf.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth, color type, compression, filter, interlace
        buf.extend_from_slice(&[0u8; 4]); // CRC placeholder (header validator checks sig+IHDR dims)
        buf
    }

    #[tokio::test]
    async fn create_auto_naming_uniqueness_and_22_agent_split_persistence() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let state = tmp.path().join("state");
        let ctrl = BotController::open(&state, home.clone()).unwrap();

        let b1 = ctrl.create_bot(CreateBotRequest::default()).await.unwrap();
        let b2 = ctrl.create_bot(CreateBotRequest::default()).await.unwrap();
        assert_eq!(b1.name.as_str(), "New bot");
        assert_eq!(b2.name.as_str(), "New bot 2");

        // Duplicate name rejected (case-insensitive)
        let dup_err = ctrl
            .create_bot(CreateBotRequest {
                name: Some("new bot".to_string()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(dup_err.code(), BotErrorCode::BotNameTaken);

        // Create bots across legacy-readable and newer agents
        let mut created_ids = vec![b1.id.clone(), b2.id.clone()];
        for agent in BotAgentId::ALL {
            let bot = ctrl
                .create_bot(CreateBotRequest {
                    name: Some(format!("Agent {}", agent.as_str())),
                    agent,
                    ..Default::default()
                })
                .await
                .unwrap();
            created_ids.push(bot.id);
        }

        let legacy_raw = read_raw_json_array(&state.join(BOTS_LEGACY_FILE)).unwrap();
        let newer_raw = read_raw_json_array(&state.join(BOTS_NEWER_FILE)).unwrap();
        assert_eq!(legacy_raw.len(), 2 + BOT_AGENTS_OLDER_BUILDS_READ.len());
        assert_eq!(
            newer_raw.len(),
            BotAgentId::ALL.len() - BOT_AGENTS_OLDER_BUILDS_READ.len()
        );

        // Inject retired "gemini" record into bots.json and verify reload migrates to Antigravity
        let mut legacy_with_retired = legacy_raw;
        legacy_with_retired.push(serde_json::json!({
            "id": "retired-1",
            "name": "Old Gemini",
            "folder": "old-gemini",
            "agent": "gemini",
            "launch": { "model": "gemini-pro" }
        }));
        fs::write(
            state.join(BOTS_LEGACY_FILE),
            serde_json::to_string_pretty(&legacy_with_retired).unwrap(),
        )
        .unwrap();

        let reloaded = BotController::open(&state, home).unwrap();
        let migrated = reloaded.get_bot("retired-1").await.unwrap();
        assert_eq!(migrated.agent, BotAgentId::Antigravity);
        assert!(migrated.launch.is_none());
        for id in created_ids {
            assert!(reloaded.get_bot(&id).await.is_ok());
        }
    }

    #[tokio::test]
    async fn serialized_concurrent_patches_host_mcp_guard_and_offer_cas() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let ctrl = BotController::open(tmp.path().join("state"), home).unwrap();

        let bot = ctrl
            .create_bot(CreateBotRequest {
                name: Some("Triage".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();

        // Concurrent functional member additions never clobber
        let c1 = ctrl.clone();
        let c2 = ctrl.clone();
        let id1 = bot.id.clone();
        let id2 = bot.id.clone();
        let (r1, r2) = tokio::join!(
            c1.update_bot(
                &id1,
                BotPatch {
                    add_members: vec![BotMember {
                        id: "u1".to_string(),
                        name: "Alice".to_string(),
                        source: Some("slack".to_string()),
                        workspace_id: Some("T1".to_string()),
                        avatar_url: None,
                        kind: None,
                    }],
                    ..Default::default()
                }
            ),
            c2.update_bot(
                &id2,
                BotPatch {
                    add_members: vec![BotMember {
                        id: "u2".to_string(),
                        name: "Bob".to_string(),
                        source: Some("slack".to_string()),
                        workspace_id: Some("T1".to_string()),
                        avatar_url: None,
                        kind: None,
                    }],
                    ..Default::default()
                }
            )
        );
        r1.unwrap();
        r2.unwrap();
        let after = ctrl.get_bot(&bot.id).await.unwrap();
        assert_eq!(after.members.len(), 2);

        // Host-only MCP toggle cannot be modified by general BotPatch
        assert!(!after.connectors.uses_host_mcp_servers());
        let enabled = ctrl.set_host_mcp_servers(&bot.id, true).await.unwrap();
        assert!(enabled.connectors.uses_host_mcp_servers());
        let patched = ctrl
            .update_bot(
                &bot.id,
                BotPatch {
                    paused: Some(true),
                    connectors: Some(BotConnectorPatch {
                        google: Some(Some(true)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(patched.paused);
        assert!(patched.connectors.uses_host_mcp_servers());
        assert_eq!(patched.connectors.google, Some(true));

        // Offer answer CAS succeeds once, rejects duplicate
        ctrl.update_bot(
            &bot.id,
            BotPatch {
                offer_answer: Some(BotOfferAnswerUpdate::Claim {
                    offer_id: "offer-1".to_string(),
                    state: BotOfferAnswerState::Sent,
                    answered_at: 2_000,
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let cas_err = ctrl
            .update_bot(
                &bot.id,
                BotPatch {
                    offer_answer: Some(BotOfferAnswerUpdate::Claim {
                        offer_id: "offer-1".to_string(),
                        state: BotOfferAnswerState::Dismissed,
                        answered_at: 3_000,
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(cas_err.code(), BotErrorCode::BotOfferAlreadyAnswered);
    }

    #[tokio::test]
    async fn avatar_cas_png_validation_and_ownership_guarded_delete() {
        let tmp = TempDir::new().unwrap();
        let home = BotHomeStore::new(tmp.path().join("bots"));
        let ctrl = BotController::open(tmp.path().join("state"), home.clone()).unwrap();

        let bot = ctrl
            .create_bot(CreateBotRequest {
                name: Some("Designer".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(bot.avatar_spec.avatar_revision, 0);

        let updated = ctrl
            .update_avatar_style(
                &bot.id,
                UpdateAvatarStyleRequest::mascot(
                    BotMascotId::D1,
                    Some("#f6c2d9".to_string()),
                    Some("artist".to_string()),
                    Some(0),
                    Some(bot.avatar_spec.avatar_revision_epoch.clone()),
                ),
            )
            .await
            .unwrap();
        assert_eq!(updated.avatar_spec.avatar_revision, 1);

        // Stale revision fails with BotAvatarChanged
        let stale_err = ctrl
            .update_avatar_style(
                &bot.id,
                UpdateAvatarStyleRequest::mascot(BotMascotId::E1, None, None, Some(0), None),
            )
            .await
            .unwrap_err();
        assert_eq!(stale_err.code(), BotErrorCode::BotAvatarChanged);

        // Invalid PNG fails with BotAvatarInvalid
        let bad_png = ctrl
            .upload_avatar_png(
                &bot.id,
                UploadAvatarPngRequest {
                    png_bytes: b"not-a-png".to_vec(),
                    expected_avatar_revision: Some(1),
                    expected_avatar_revision_epoch: None,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(bad_png.code(), BotErrorCode::BotAvatarInvalid);

        // Valid 512x512 PNG succeeds, writes avatar.png, bumps revision, mirrors in bot.json
        let valid_png = make_valid_png(512, 512);
        let uploaded = ctrl
            .upload_avatar_png(
                &bot.id,
                UploadAvatarPngRequest {
                    png_bytes: valid_png.clone(),
                    expected_avatar_revision: Some(1),
                    expected_avatar_revision_epoch: Some(
                        updated.avatar_spec.avatar_revision_epoch.clone(),
                    ),
                },
            )
            .await
            .unwrap();
        assert_eq!(uploaded.avatar_spec.avatar_revision, 2);
        assert!(uploaded.avatar_spec.custom_png);
        assert_eq!(
            uploaded.avatar_spec.generated_over_mascot.as_deref(),
            Some("mascot:D1")
        );
        assert_eq!(
            fs::read(home.bot_dir(&uploaded.folder).join("avatar.png")).unwrap(),
            valid_png
        );
        let manifest = home.read_bot_manifest(&uploaded.folder).unwrap().unwrap();
        assert_eq!(manifest["avatar_revision"], 2);

        // Delete removes folder and bot record
        let release = ctrl.delete_bot(&bot.id).await.unwrap();
        assert_eq!(release.bot_id, bot.id);
        assert!(!home.bot_dir(&uploaded.folder).exists());
        assert_eq!(
            ctrl.get_bot(&bot.id).await.unwrap_err().code(),
            BotErrorCode::BotNotFound
        );
    }
}
