//! Bot home directory store: slug validation, `bot.json` ownership guard,
//! atomic CAS filesystem primitives, generated/seeded files, and skill symlinks.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use orca_core::bot_preamble::{
    generated_bot_home_files, seeded_bot_home_files, verify_bot_home_ownership,
};
use orca_core::bot_types::{
    BotFolderSlug, OrcaBotRecord, bot_folder_name, validate_bot_avatar_png,
};
use orca_core::BotErrorCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{OrcaBotsError, Result, now_epoch_ms};

const DEFAULT_BOTS_SUBPATH: [&str; 2] = [".fabushi", "bots"];
const MAX_FOLDER_SLUG_LEN: usize = 40;
const MAX_UNIQUE_FOLDER_ATTEMPTS: usize = 10_000;

/// Metadata returned when a bot's home directory is verified and released on deletion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeReleaseBotHome {
    pub bot_id: String,
    pub folder: BotFolderSlug,
    pub home_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_workspace_id: Option<String>,
    pub released_at_ms: i64,
}

/// Manages `<bots_root>/<slug>` home directories with strict ownership & atomic writes.
#[derive(Debug, Clone)]
pub struct BotHomeStore {
    root: PathBuf,
    cas_lock: Arc<Mutex<()>>,
}

impl BotHomeStore {
    /// Resolves the default bots root directory (`~/.fabushi/bots`).
    pub fn default_root() -> PathBuf {
        let base = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("fabushi-home"));
        base.join(DEFAULT_BOTS_SUBPATH[0])
            .join(DEFAULT_BOTS_SUBPATH[1])
    }

    /// Creates a store rooted at `~/.fabushi/bots`.
    pub fn new_default() -> Self {
        Self::new(Self::default_root())
    }

    /// Creates a store rooted at an explicit directory (for tests or isolated profiles).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            cas_lock: Arc::new(Mutex::new(())),
        }
    }

    /// Returns the configured root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves the absolute directory path for a validated `BotFolderSlug`.
    pub fn bot_dir(&self, folder: &BotFolderSlug) -> PathBuf {
        self.root.join(folder.as_str())
    }

    /// Validates a raw slug string via `BotFolderSlug::new` and returns its bot directory path.
    pub fn bot_dir_from_str(&self, raw_folder: &str) -> Result<PathBuf> {
        let slug = BotFolderSlug::new(raw_folder)?;
        Ok(self.bot_dir(&slug))
    }

    /// Resolves a safe relative path strictly inside `<bots_root>/<folder>`.
    pub fn resolve_relative_path(&self, folder: &BotFolderSlug, rel: &str) -> Result<PathBuf> {
        if rel.is_empty() || rel.contains('\0') || rel.contains('\\') {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Relative bot home path cannot be empty or contain NUL/backslashes.",
            ));
        }
        let rel_path = Path::new(rel);
        if rel_path.is_absolute() {
            return Err(OrcaBotsError::bot(
                BotErrorCode::InvalidArgument,
                "Bot home path must be relative.",
            ));
        }
        let mut out = self.bot_dir(folder);
        for comp in rel_path.components() {
            match comp {
                Component::Normal(part) => out.push(part),
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(OrcaBotsError::bot(
                        BotErrorCode::InvalidArgument,
                        format!("Disallowed path traversal component in '{rel}'."),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// Atomically reserves `<bots_root>/<folder>` via `create_dir`, failing on `AlreadyExists`.
    pub fn reserve_directory(&self, folder: &BotFolderSlug) -> Result<PathBuf> {
        fs::create_dir_all(&self.root)?;
        let dir = self.bot_dir(folder);
        fs::create_dir(&dir)?;
        Ok(dir)
    }

    /// Derives a slug from `bot_name` and atomically reserves the first free folder
    /// (`<slug>`, `<slug>-2`, `<slug>-3`, ...) via `reserve_directory`.
    pub fn reserve_unique_folder(&self, bot_name: &str) -> Result<(BotFolderSlug, PathBuf)> {
        let base = bot_folder_name(bot_name);
        match self.reserve_directory(&base) {
            Ok(dir) => return Ok((base, dir)),
            Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }

        for n in 2..=MAX_UNIQUE_FOLDER_ATTEMPTS {
            let suffix = format!("-{n}");
            let max_prefix = MAX_FOLDER_SLUG_LEN.saturating_sub(suffix.len());
            let trimmed_prefix = base
                .as_str()
                .chars()
                .take(max_prefix)
                .collect::<String>()
                .trim_end_matches('-')
                .to_string();
            let candidate_str = if trimmed_prefix.is_empty() {
                format!("bot{suffix}")
            } else {
                format!("{trimmed_prefix}{suffix}")
            };
            let slug = BotFolderSlug::new(&candidate_str)?;
            match self.reserve_directory(&slug) {
                Ok(dir) => return Ok((slug, dir)),
                Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {
                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        Err(OrcaBotsError::bot(
            BotErrorCode::Full,
            format!("Unable to reserve a unique bot folder for '{bot_name}'."),
        ))
    }

    /// Reads an optional UTF-8 text file, returning `Ok(None)` when absent.
    pub fn read_text_optional(&self, path: &Path) -> Result<Option<String>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(OrcaBotsError::Io(err)),
        }
    }

    /// Atomically writes `bytes` to `path` via sibling temp file + `sync_all()` + rename.
    pub fn replace_bytes(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        atomic_write_bytes(path, bytes)
    }

    /// Creates a new text file exclusively (`create_new(true)`), failing if it already exists.
    pub fn write_text_exclusive(&self, path: &Path, text: &str) -> Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }

    /// Compare-and-swap text replacement: checks the live file on disk matches `expected_baseline`
    /// (`None` requires the file not to exist) before atomically swapping in `new_text`.
    pub fn replace_text(
        &self,
        path: &Path,
        expected_baseline: Option<&str>,
        new_text: &str,
    ) -> Result<()> {
        let _guard = self.cas_lock.lock().map_err(|_| {
            OrcaBotsError::bot(BotErrorCode::InvalidArgument, "BotHomeStore CAS lock poisoned.")
        })?;

        let current = self.read_text_optional(path)?;
        let matches = match (current.as_deref(), expected_baseline) {
            (None, None) => true,
            (Some(disk), Some(exp)) => {
                disk.len() == exp.len()
                    && Sha256::digest(disk.as_bytes()) == Sha256::digest(exp.as_bytes())
                    && disk == exp
            }
            _ => false,
        };
        if !matches {
            return Err(OrcaBotsError::PreconditionFailed {
                path: path.display().to_string(),
            });
        }

        if expected_baseline.is_none() {
            match self.write_text_exclusive(path, new_text) {
                Ok(()) => Ok(()),
                Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {
                    Err(OrcaBotsError::PreconditionFailed {
                        path: path.display().to_string(),
                    })
                }
                Err(err) => Err(err),
            }
        } else {
            atomic_write_bytes(path, new_text.as_bytes())
        }
    }

    /// Strict ownership check: `bot.json` MUST exist, parse as valid JSON with a string `"id"`,
    /// and match `expected_bot_id`, else returns `BotErrorCode::BotFolderNotOwned`.
    pub fn assert_bot_owns_home(
        &self,
        folder: &BotFolderSlug,
        expected_bot_id: &str,
    ) -> Result<()> {
        let manifest_path = self.bot_dir(folder).join("bot.json");
        let raw = match fs::read_to_string(&manifest_path) {
            Ok(content) => content,
            Err(_) => {
                return Err(OrcaBotsError::bot(
                    BotErrorCode::BotFolderNotOwned,
                    format!(
                        "Bot folder '{}' is missing a readable bot.json for bot '{}'.",
                        folder.as_str(),
                        expected_bot_id
                    ),
                ));
            }
        };

        let val: serde_json::Value = serde_json::from_str(&raw).map_err(|_| {
            OrcaBotsError::bot(
                BotErrorCode::BotFolderNotOwned,
                format!("Bot folder '{}' has corrupt bot.json.", folder.as_str()),
            )
        })?;

        let found_id = val.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
            OrcaBotsError::bot(
                BotErrorCode::BotFolderNotOwned,
                format!("Bot folder '{}' bot.json is missing 'id'.", folder.as_str()),
            )
        })?;

        verify_bot_home_ownership(Some(&raw), expected_bot_id)?;
        if found_id != expected_bot_id {
            return Err(OrcaBotsError::bot(
                BotErrorCode::BotFolderNotOwned,
                format!(
                    "Bot folder '{}' belongs to '{}', not '{}'.",
                    folder.as_str(),
                    found_id,
                    expected_bot_id
                ),
            ));
        }
        Ok(())
    }

    /// Provisions a bot home directory: creates/verifies the directory, `memory/`, `skills/`,
    /// one-time seeded `SOUL.md` & `MEMORY.md`, generated `AGENTS.md`/`CLAUDE.md`/`bot.json`,
    /// and `.claude/skills` + `.agents/skills` symlinks.
    pub fn provision_bot_home(
        &self,
        bot: &OrcaBotRecord,
        custom_soul: Option<&str>,
    ) -> Result<PathBuf> {
        let dir = self.bot_dir(&bot.folder);
        if !dir.exists() {
            self.reserve_directory(&bot.folder)?;
        } else if !is_dir_empty(&dir)? {
            self.assert_bot_owns_home(&bot.folder, &bot.id)?;
        }

        fs::create_dir_all(dir.join("memory"))?;
        fs::create_dir_all(dir.join("skills"))?;

        for seeded in seeded_bot_home_files(bot, custom_soul) {
            let target = self.resolve_relative_path(&bot.folder, &seeded.path)?;
            match self.write_text_exclusive(&target, &seeded.text) {
                Ok(()) => {}
                Err(OrcaBotsError::Io(ref err)) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }

        for gen_file in generated_files_with_folder(bot)? {
            let target = self.resolve_relative_path(&bot.folder, &gen_file.path)?;
            atomic_write_bytes(&target, gen_file.text.as_bytes())?;
        }

        self.sync_skill_symlinks(&bot.folder)?;
        Ok(dir)
    }

    /// Refreshes host-owned `AGENTS.md`, `CLAUDE.md`, `bot.json`, and skill symlinks
    /// only after confirming `bot.json` ownership.
    pub fn sync_generated_home_files(&self, bot: &OrcaBotRecord) -> Result<()> {
        self.assert_bot_owns_home(&bot.folder, &bot.id)?;
        let dir = self.bot_dir(&bot.folder);
        fs::create_dir_all(dir.join("memory"))?;
        fs::create_dir_all(dir.join("skills"))?;

        for gen_file in generated_files_with_folder(bot)? {
            let target = self.resolve_relative_path(&bot.folder, &gen_file.path)?;
            let current = self.read_text_optional(&target)?;
            if current.as_deref() != Some(gen_file.text.as_str()) {
                atomic_write_bytes(&target, gen_file.text.as_bytes())?;
            }
        }

        self.sync_skill_symlinks(&bot.folder)?;
        Ok(())
    }

    /// Idempotently links `.claude/skills` and `.agents/skills` to `../skills`.
    pub fn sync_skill_symlinks(&self, folder: &BotFolderSlug) -> Result<()> {
        let dir = self.bot_dir(folder);
        fs::create_dir_all(dir.join("skills"))?;

        for dot_dir in [".claude", ".agents"] {
            let parent = dir.join(dot_dir);
            fs::create_dir_all(&parent)?;
            let link_path = parent.join("skills");
            ensure_skill_symlink(&link_path, Path::new("../skills"))?;
        }
        Ok(())
    }

    /// Validates and atomically writes `avatar.png` after verifying home ownership.
    pub fn write_avatar_png(
        &self,
        folder: &BotFolderSlug,
        expected_bot_id: &str,
        png_bytes: &[u8],
    ) -> Result<PathBuf> {
        self.assert_bot_owns_home(folder, expected_bot_id)?;
        validate_bot_avatar_png(png_bytes)?;
        let avatar_path = self.bot_dir(folder).join("avatar.png");
        self.replace_bytes(&avatar_path, png_bytes)?;
        Ok(avatar_path)
    }

    /// Reads and parses `<home>/bot.json` if present.
    pub fn read_bot_manifest(&self, folder: &BotFolderSlug) -> Result<Option<serde_json::Value>> {
        let manifest_path = self.bot_dir(folder).join("bot.json");
        let Some(raw) = self.read_text_optional(&manifest_path)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&raw)?))
    }

    /// Verifies `bot.json` ownership and removes the bot's home directory, returning release info.
    pub fn release_bot_home(
        &self,
        folder: &BotFolderSlug,
        bot_id: &str,
        folder_workspace_id: Option<String>,
    ) -> Result<WorktreeReleaseBotHome> {
        self.assert_bot_owns_home(folder, bot_id)?;
        let home_path = self.bot_dir(folder);
        fs::remove_dir_all(&home_path)?;
        Ok(WorktreeReleaseBotHome {
            bot_id: bot_id.to_string(),
            folder: folder.clone(),
            home_path,
            folder_workspace_id,
            released_at_ms: now_epoch_ms(),
        })
    }
}

fn is_dir_empty(dir: &Path) -> Result<bool> {
    let mut entries = fs::read_dir(dir)?;
    Ok(entries.next().is_none())
}

fn generated_files_with_folder(
    bot: &OrcaBotRecord,
) -> Result<Vec<orca_core::bot_preamble::BotHomeFile>> {
    let mut files = generated_bot_home_files(bot);
    for file in &mut files {
        if file.path == "bot.json" {
            let mut val: serde_json::Value = serde_json::from_str(&file.text)?;
            if let Some(obj) = val.as_object_mut() {
                obj.insert(
                    "folder".to_string(),
                    serde_json::Value::String(bot.folder.as_str().to_string()),
                );
            }
            let mut pretty = serde_json::to_string_pretty(&val)?;
            pretty.push('\n');
            file.text = pretty;
        }
    }
    Ok(files)
}

#[cfg(unix)]
fn ensure_skill_symlink(link_path: &Path, target: &Path) -> Result<()> {
    match fs::symlink_metadata(link_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                if let Ok(existing_target) = fs::read_link(link_path)
                    && existing_target == target
                {
                    return Ok(());
                }
                fs::remove_file(link_path)?;
            } else if meta.is_dir() {
                fs::remove_dir_all(link_path)?;
            } else {
                fs::remove_file(link_path)?;
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(OrcaBotsError::Io(err)),
    }
    std::os::unix::fs::symlink(target, link_path)?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_skill_symlink(link_path: &Path, _target: &Path) -> Result<()> {
    fs::create_dir_all(link_path)?;
    Ok(())
}

/// Atomically writes `bytes` to `path` via a unique sibling temp file, `sync_all()`, and rename.
pub(crate) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    let tmp_path = parent.join(format!(".tmp.{file_name}.{}", Uuid::new_v4()));

    let write_res = (|| -> io::Result<()> {
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        tmp_file.write_all(bytes)?;
        tmp_file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(OrcaBotsError::Io(err));
    }

    if let Err(err) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(OrcaBotsError::Io(err));
    }

    if let Ok(dir) = File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::bot_preamble::BOT_MEMORY_SEED;
    use orca_core::bot_types::{
        BotAgentId, BotAvatarSpec, BotConnectorLinks, BotMascotId, BotName, BotNotifications,
    };
    use orca_core::LOCAL_EXECUTION_HOST_ID;
    use tempfile::TempDir;

    fn sample_bot(id: &str, name: &str, folder: &str) -> OrcaBotRecord {
        OrcaBotRecord {
            id: id.to_string(),
            name: BotName::new(name).unwrap(),
            host_id: LOCAL_EXECUTION_HOST_ID.to_string(),
            folder: BotFolderSlug::new(folder).unwrap(),
            folder_workspace_id: Some("ws-bots".to_string()),
            agent: BotAgentId::Claude,
            avatar_spec: BotAvatarSpec::new_with_mascot(BotMascotId::A1),
            paused: false,
            permission_mode: None,
            launch: None,
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
            first_run: true,
            accepted_offers: None,
            offer_answers: None,
            handed_paired_workspaces: None,
            cancelled_waits: None,
            imported_from: None,
            shared_from: None,
            template_id: None,
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    #[test]
    fn default_root_ends_with_fabushi_bots_and_slug_rejects_traversal() {
        let root = BotHomeStore::default_root();
        assert!(root.ends_with(Path::new(".fabushi").join("bots")));

        let tmp = TempDir::new().unwrap();
        let store = BotHomeStore::new(tmp.path());
        assert_eq!(
            store.bot_dir_from_str("../evil").unwrap_err().code(),
            BotErrorCode::InvalidArgument
        );
        let slug = BotFolderSlug::new("helper-bot").unwrap();
        assert_eq!(
            store.resolve_relative_path(&slug, "../outside").unwrap_err().code(),
            BotErrorCode::InvalidArgument
        );
        assert_eq!(
            store.resolve_relative_path(&slug, "/abs/path").unwrap_err().code(),
            BotErrorCode::InvalidArgument
        );
    }

    #[test]
    fn reserve_directory_and_atomic_file_cas_primitives() {
        let tmp = TempDir::new().unwrap();
        let store = BotHomeStore::new(tmp.path());
        let slug = BotFolderSlug::new("release-bot").unwrap();

        let dir = store.reserve_directory(&slug).unwrap();
        assert!(dir.is_dir());
        let dup_err = store.reserve_directory(&slug).unwrap_err();
        assert!(matches!(dup_err, OrcaBotsError::Io(e) if e.kind() == io::ErrorKind::AlreadyExists));

        let (slug2, _) = store.reserve_unique_folder("Release Bot").unwrap();
        assert_eq!(slug2.as_str(), "release-bot-2");

        let note = dir.join("note.md");
        store.write_text_exclusive(&note, "v1").unwrap();
        assert!(matches!(
            store.write_text_exclusive(&note, "v2").unwrap_err(),
            OrcaBotsError::Io(e) if e.kind() == io::ErrorKind::AlreadyExists
        ));

        assert!(matches!(
            store.replace_text(&note, Some("wrong"), "v2").unwrap_err(),
            OrcaBotsError::PreconditionFailed { .. }
        ));
        store.replace_text(&note, Some("v1"), "v2").unwrap();
        assert_eq!(store.read_text_optional(&note).unwrap().as_deref(), Some("v2"));

        let bin = dir.join("blob.bin");
        store.replace_bytes(&bin, b"hello-atomic").unwrap();
        assert_eq!(fs::read(&bin).unwrap(), b"hello-atomic");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp."))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn provision_ownership_symlinks_and_release_guard() {
        let tmp = TempDir::new().unwrap();
        let store = BotHomeStore::new(tmp.path());
        let bot = sample_bot("bot-1", "Ops Bot", "ops-bot");

        let dir = store.provision_bot_home(&bot, None).unwrap();
        assert!(dir.join("SOUL.md").is_file());
        assert_eq!(
            fs::read_to_string(dir.join("MEMORY.md")).unwrap(),
            BOT_MEMORY_SEED
        );
        assert!(dir.join("AGENTS.md").is_file());
        assert_eq!(
            fs::read_to_string(dir.join("CLAUDE.md")).unwrap(),
            "@AGENTS.md\n"
        );
        assert!(dir.join("memory").is_dir());
        assert!(dir.join("skills").is_dir());

        let manifest = store.read_bot_manifest(&bot.folder).unwrap().unwrap();
        assert_eq!(manifest["id"], "bot-1");
        assert_eq!(manifest["name"], "Ops Bot");
        assert_eq!(manifest["folder"], "ops-bot");
        assert_eq!(manifest["avatar_revision"], 0);
        assert_eq!(
            manifest["avatar_revision_epoch"],
            bot.avatar_spec.avatar_revision_epoch
        );

        #[cfg(unix)]
        {
            let expected = fs::canonicalize(dir.join("skills")).unwrap();
            assert_eq!(
                fs::canonicalize(dir.join(".claude").join("skills")).unwrap(),
                expected
            );
            assert_eq!(
                fs::canonicalize(dir.join(".agents").join("skills")).unwrap(),
                expected
            );
            // Idempotent re-sync
            store.sync_skill_symlinks(&bot.folder).unwrap();
            assert_eq!(
                fs::canonicalize(dir.join(".claude").join("skills")).unwrap(),
                expected
            );
        }

        // User edits SOUL.md -> re-provisioning preserves user edits
        fs::write(dir.join("SOUL.md"), "# Custom Soul\n").unwrap();
        store.provision_bot_home(&bot, Some("Ignored")).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("SOUL.md")).unwrap(),
            "# Custom Soul\n"
        );

        // Ownership guard rejects wrong bot ID, corrupt bot.json, and missing bot.json
        assert_eq!(
            store.assert_bot_owns_home(&bot.folder, "other-bot").unwrap_err().code(),
            BotErrorCode::BotFolderNotOwned
        );
        assert_eq!(
            store.release_bot_home(&bot.folder, "other-bot", None).unwrap_err().code(),
            BotErrorCode::BotFolderNotOwned
        );
        assert!(dir.exists());

        fs::write(dir.join("bot.json"), "{not-json").unwrap();
        assert_eq!(
            store.assert_bot_owns_home(&bot.folder, "bot-1").unwrap_err().code(),
            BotErrorCode::BotFolderNotOwned
        );
        fs::remove_file(dir.join("bot.json")).unwrap();
        assert_eq!(
            store.assert_bot_owns_home(&bot.folder, "bot-1").unwrap_err().code(),
            BotErrorCode::BotFolderNotOwned
        );

        // Restore ownership & release cleanly
        fs::write(dir.join("bot.json"), r#"{"id":"bot-1"}"#).unwrap();
        let release = store
            .release_bot_home(&bot.folder, "bot-1", Some("ws-bots".to_string()))
            .unwrap();
        assert_eq!(release.bot_id, "bot-1");
        assert_eq!(release.folder, bot.folder);
        assert!(!dir.exists());
    }
}
