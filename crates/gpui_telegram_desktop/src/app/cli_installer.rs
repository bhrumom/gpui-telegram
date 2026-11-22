//! First-run & one-click `fabushi` CLI PATH auto-installer for Desktop app users.
//!
//! Ensures users who install `Fabushi.app` via DMG drag-and-drop (bypassing `.pkg` postinstall)
//! still get `fabushi` available in `/usr/local/bin/fabushi` or `~/.local/bin/fabushi`.

use std::path::{Path, PathBuf};

/// Resolved installation and PATH status of the `fabushi` CLI binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliInstallStatus {
    pub installed: bool,
    pub on_path: bool,
    pub installed_path: Option<PathBuf>,
    pub bundled_cli_path: Option<PathBuf>,
    pub status_message: String,
}

impl CliInstallStatus {
    pub fn display_path(&self) -> String {
        self.installed_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| {
                if cfg!(windows) {
                    "%LOCALAPPDATA%\\Programs\\Fabushi\\bin\\fabushi.exe".to_string()
                } else {
                    "~/.local/bin/fabushi".to_string()
                }
            })
    }
}

/// Locates the bundled `fabushi` CLI binary relative to the running executable.
pub fn locate_bundled_fabushi_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    locate_bundled_fabushi_cli_from_exe(&exe)
}

pub fn locate_bundled_fabushi_cli_from_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;
    let bin_name = if cfg!(windows) {
        "fabushi.exe"
    } else {
        "fabushi"
    };

    let candidates = [
        // macOS .app bundle: Contents/MacOS/<exe> -> Contents/Helpers/fabushi
        exe_dir.join("../Helpers").join(bin_name),
        exe_dir.join("../Resources").join(bin_name),
        // Windows / Linux: alongside main executable or in bin/
        exe_dir.join("bin").join(bin_name),
        exe_dir.join(bin_name),
    ];

    for cand in candidates {
        if cand.is_file() {
            return Some(cand.canonicalize().unwrap_or(cand));
        }
    }
    None
}

fn default_user_local_bin_target() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|base| {
            PathBuf::from(base)
                .join("Programs")
                .join("Fabushi")
                .join("bin")
                .join("fabushi.exe")
        })
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("bin")
                .join("fabushi")
        })
    }
}

fn default_candidate_install_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(not(windows))]
    {
        paths.push(PathBuf::from("/usr/local/bin/fabushi"));
        paths.push(PathBuf::from("/opt/homebrew/bin/fabushi"));
    }
    if let Some(user_target) = default_user_local_bin_target() {
        paths.push(user_target);
    }
    paths
}

fn check_dir_in_path(dir: &Path, path_env: &str) -> bool {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let target_canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    path_env
        .split(sep)
        .filter(|s| !s.trim().is_empty())
        .any(|entry| {
            let p = Path::new(entry.trim());
            p == dir || p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) == target_canon
        })
}

fn find_in_path(bin_name: &str, path_env: &str) -> Option<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    for dir in path_env.split(sep).filter(|s| !s.trim().is_empty()) {
        let cand = Path::new(dir.trim()).join(bin_name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Evaluates CLI installation & PATH status using explicit candidate paths (pure/testable).
pub fn inspect_cli_status_with(
    bundled_cli_path: Option<PathBuf>,
    candidate_paths: &[PathBuf],
    path_env: &str,
) -> CliInstallStatus {
    let bin_name = if cfg!(windows) {
        "fabushi.exe"
    } else {
        "fabushi"
    };

    if let Some(in_path) = find_in_path(bin_name, path_env) {
        let msg = format!("CLI ready on PATH ({})", in_path.display());
        return CliInstallStatus {
            installed: true,
            on_path: true,
            installed_path: Some(in_path),
            bundled_cli_path,
            status_message: msg,
        };
    }

    for cand in candidate_paths {
        if cand.exists() {
            let on_path = cand
                .parent()
                .is_some_and(|dir| check_dir_in_path(dir, path_env));
            let msg = if on_path {
                format!("CLI ready ({})", cand.display())
            } else {
                format!(
                    "Installed at {} (add its folder to PATH)",
                    cand.display()
                )
            };
            return CliInstallStatus {
                installed: true,
                on_path,
                installed_path: Some(cand.clone()),
                bundled_cli_path,
                status_message: msg,
            };
        }
    }

    let status_message = if bundled_cli_path.is_some() {
        "CLI not installed in PATH yet".to_string()
    } else {
        "Bundled fabushi CLI binary not found (run from app bundle or cargo)".to_string()
    };

    CliInstallStatus {
        installed: false,
        on_path: false,
        installed_path: None,
        bundled_cli_path,
        status_message,
    }
}

/// Checks `/usr/local/bin/fabushi` & `~/.local/bin/fabushi` and `PATH` for the `fabushi` CLI.
pub fn detect_cli_install_status() -> CliInstallStatus {
    let bundled = locate_bundled_fabushi_cli();
    let candidates = default_candidate_install_paths();
    let path_env = std::env::var("PATH").unwrap_or_default();
    inspect_cli_status_with(bundled, &candidates, &path_env)
}

/// Links (Unix) or copies (Windows) `bundled_cli` to `target_path`, creating parent directories.
pub fn install_cli_to_target(bundled_cli: &Path, target_path: &Path) -> Result<PathBuf, String> {
    if !bundled_cli.is_file() {
        return Err(format!(
            "Bundled CLI binary does not exist: {}",
            bundled_cli.display()
        ));
    }
    if let Some(parent) = target_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
    }

    if let Ok(meta) = std::fs::symlink_metadata(target_path) {
        if meta.file_type().is_symlink() || meta.is_file() {
            let _ = std::fs::remove_file(target_path);
        }
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(bundled_cli, target_path).map_err(|e| {
            format!(
                "Failed to symlink {} -> {}: {e}",
                target_path.display(),
                bundled_cli.display()
            )
        })?;
    }

    #[cfg(not(unix))]
    {
        std::fs::copy(bundled_cli, target_path).map_err(|e| {
            format!(
                "Failed to copy {} -> {}: {e}",
                bundled_cli.display(),
                target_path.display()
            )
        })?;
    }

    Ok(target_path.to_path_buf())
}

/// Silent startup check: if `fabushi` is missing from `/usr/local/bin` and `~/.local/bin`
/// and a bundled CLI exists inside the app bundle, silently link it without prompting.
pub fn startup_ensure_cli_linked_if_bundled() -> CliInstallStatus {
    let initial = detect_cli_install_status();
    if cfg!(test) || std::env::var_os("GPUI_OFFLINE_TEST").is_some() {
        return initial;
    }
    if initial.installed {
        return initial;
    }
    let Some(ref bundled) = initial.bundled_cli_path else {
        return initial;
    };

    #[cfg(not(windows))]
    {
        let sys_target = Path::new("/usr/local/bin/fabushi");
        if install_cli_to_target(bundled, sys_target).is_ok() {
            return detect_cli_install_status();
        }
    }

    if let Some(user_target) = default_user_local_bin_target() {
        let _ = install_cli_to_target(bundled, &user_target);
    }
    detect_cli_install_status()
}

/// One-click install invoked from Settings -> AI & Official MCP Authorization Center.
/// Attempts `/usr/local/bin/fabushi` first (using a native macOS admin prompt only when
/// unprivileged write fails and not running headless/offline), and falls back to `~/.local/bin/fabushi`.
pub fn install_fabushi_cli_one_click() -> Result<CliInstallStatus, String> {
    let status = detect_cli_install_status();
    let bundled = status.bundled_cli_path.as_ref().ok_or_else(|| {
        "Bundled fabushi binary was not found next to this executable".to_string()
    })?;

    #[cfg(not(windows))]
    {
        let sys_target = Path::new("/usr/local/bin/fabushi");
        if install_cli_to_target(bundled, sys_target).is_ok() {
            return Ok(detect_cli_install_status());
        }

        #[cfg(target_os = "macos")]
        if !cfg!(test) && std::env::var_os("GPUI_OFFLINE_TEST").is_none() {
            let escaped_src = bundled.to_string_lossy().replace('\'', "'\\''");
            let shell_cmd = format!(
                "mkdir -p /usr/local/bin && ln -sf '{}' /usr/local/bin/fabushi",
                escaped_src
            );
            let apple_script = format!(
                "do shell script \"{}\" with administrator privileges",
                shell_cmd.replace('\\', "\\\\").replace('"', "\\\"")
            );
            if let Ok(out) = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&apple_script)
                .output()
            {
                if out.status.success() {
                    return Ok(detect_cli_install_status());
                }
            }
        }
    }

    let user_target = default_user_local_bin_target()
        .ok_or_else(|| "Unable to determine user local bin directory".to_string())?;
    install_cli_to_target(bundled, &user_target)?;
    Ok(detect_cli_install_status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_installer_detection_and_symlink_install() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundled = tmp.path().join("Fabushi.app/Contents/Helpers/fabushi");
        std::fs::create_dir_all(bundled.parent().unwrap()).unwrap();
        std::fs::write(&bundled, b"#!/bin/sh\necho fabushi\n").unwrap();

        let exe = tmp.path().join("Fabushi.app/Contents/MacOS/Fabushi");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"bin").unwrap();

        let detected_bundled = locate_bundled_fabushi_cli_from_exe(&exe).expect("bundled cli");
        assert_eq!(
            detected_bundled.canonicalize().unwrap(),
            bundled.canonicalize().unwrap()
        );

        let user_bin = tmp.path().join("home/.local/bin");
        let target = user_bin.join("fabushi");
        let empty_path = tmp.path().join("empty_bin");
        std::fs::create_dir_all(&empty_path).unwrap();

        // Not installed initially
        let st0 = inspect_cli_status_with(
            Some(bundled.clone()),
            std::slice::from_ref(&target),
            &empty_path.display().to_string(),
        );
        assert!(!st0.installed);
        assert!(!st0.on_path);

        // Install symlink into user_bin
        let installed = install_cli_to_target(&bundled, &target).expect("install symlink");
        assert_eq!(installed, target);
        assert!(target.exists());

        // Installed, not on PATH
        let st1 = inspect_cli_status_with(
            Some(bundled.clone()),
            std::slice::from_ref(&target),
            &empty_path.display().to_string(),
        );
        assert!(st1.installed);
        assert!(!st1.on_path);

        // Installed and on PATH
        let st2 = inspect_cli_status_with(
            Some(bundled),
            std::slice::from_ref(&target),
            &user_bin.display().to_string(),
        );
        assert!(st2.installed);
        assert!(st2.on_path);
        assert_eq!(st2.installed_path.as_deref(), Some(target.as_path()));
    }

    #[test]
    fn test_install_cli_missing_bundled_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("missing-fabushi");
        let target = tmp.path().join("bin/fabushi");
        let err = install_cli_to_target(&missing, &target).unwrap_err();
        assert!(err.contains("does not exist"));
    }
}
