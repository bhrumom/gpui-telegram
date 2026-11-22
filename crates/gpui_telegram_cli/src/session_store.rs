use crate::error::CliError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CliSession {
    pub server_url: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub saved_at: String,
}

pub fn resolve_session_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("FABUSHI_CLI_SESSION_PATH") {
        let trimmed = explicit.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(fabushi_home) = std::env::var("FABUSHI_HOME") {
        let trimmed = fabushi_home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("cli_session.json");
        }
    }
    let home = std::env::var("HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| ".".to_string());
    PathBuf::from(home).join(".fabushi").join("cli_session.json")
}

pub fn load_cli_session() -> Option<CliSession> {
    load_cli_session_from(&resolve_session_path())
}

pub fn load_cli_session_from(path: &Path) -> Option<CliSession> {
    let raw = fs::read_to_string(path).ok()?;
    let parsed: CliSession = serde_json::from_str(&raw).ok()?;
    if parsed.token.trim().is_empty() {
        return None;
    }
    Some(parsed)
}

pub fn save_cli_session(session: &CliSession) -> Result<PathBuf, CliError> {
    let path = resolve_session_path();
    save_cli_session_to(&path, session)?;
    Ok(path)
}

pub fn save_cli_session_to(path: &Path, session: &CliSession) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            CliError::Execution(format!(
                "Failed to create session directory '{}': {e}",
                parent.display()
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let serialized = serde_json::to_string_pretty(session)
        .map_err(|e| CliError::Execution(format!("Failed to serialize CLI session: {e}")))?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| {
                CliError::Execution(format!(
                    "Failed to open CLI session file '{}': {e}",
                    path.display()
                ))
            })?;
        file.write_all(serialized.as_bytes()).map_err(|e| {
            CliError::Execution(format!(
                "Failed to write CLI session file '{}': {e}",
                path.display()
            ))
        })?;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    {
        fs::write(path, serialized.as_bytes()).map_err(|e| {
            CliError::Execution(format!(
                "Failed to write CLI session file '{}': {e}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

pub fn clear_cli_session() -> Result<bool, CliError> {
    clear_cli_session_at(&resolve_session_path())
}

pub fn clear_cli_session_at(path: &Path) -> Result<bool, CliError> {
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(path).map_err(|e| {
        CliError::Execution(format!(
            "Failed to remove CLI session file '{}': {e}",
            path.display()
        ))
    })?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cli_session_roundtrip_and_permissions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("cli_session.json");
        let sess = CliSession {
            server_url: "http://127.0.0.1:18080".into(),
            token: "tok_secret_123".into(),
            user_id: Some(42),
            username: Some("alice".into()),
            session_id: Some("sess_1".into()),
            device_id: Some("dev_1".into()),
            saved_at: "2026-11-22T00:00:00Z".into(),
        };

        save_cli_session_to(&path, &sess).unwrap();
        let loaded = load_cli_session_from(&path).unwrap();
        assert_eq!(loaded, sess);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        assert!(clear_cli_session_at(&path).unwrap());
        assert!(load_cli_session_from(&path).is_none());
        assert!(!clear_cli_session_at(&path).unwrap());
    }
}
