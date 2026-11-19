use std::cmp::Ordering;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_telegram_desktop::app::{
    compare_semver, is_newer_version, parse_github_release_json, parse_release_manifest,
    AvailableUpdateInfo, DesktopUpdateState, DesktopUpdater, UpdaterNetworkOptions,
    CURRENT_APP_VERSION,
};
use gpui_telegram_desktop::views::boxes::AboutBox;
use gpui_telegram_desktop::views::settings::sections::SettingsAdvancedSections;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn create_mock_app_bundle(root: &Path, version: &str) -> std::path::PathBuf {
    let app = root.join("Fabushi.app");
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).unwrap();
    fs::create_dir_all(&resources).unwrap();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Fabushi</string>
    <key>CFBundleDisplayName</key>
    <string>Fabushi</string>
    <key>CFBundleIdentifier</key>
    <string>com.ombhrum.fabushi</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleExecutable</key>
    <string>gpui_telegram_desktop</string>
</dict>
</plist>
"#
    );
    fs::write(contents.join("Info.plist"), plist).unwrap();

    let exe = macos.join("gpui_telegram_desktop");
    let exe_script = format!("#!/bin/sh\necho \"Fabushi Desktop v{version}\"\n");
    fs::write(&exe, exe_script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
    }
    app
}

fn sha256_hex(path: &Path) -> String {
    let bytes = fs::read(path).unwrap();
    let digest = Sha256::digest(&bytes);
    format!("{:x}", digest)
}

fn create_mock_app_zip(root: &Path, version: &str) -> (std::path::PathBuf, Vec<u8>, String) {
    let build_dir = root.join(format!("build_v{}", version.replace('.', "_")));
    let upgraded_app = create_mock_app_bundle(&build_dir, version);
    let zip_path = root.join(format!("Fabushi-{version}-macos-arm64.zip"));
    let zip_ok = if Path::new("/usr/bin/ditto").exists() {
        Command::new("/usr/bin/ditto")
            .args([
                "-c",
                "-k",
                "--sequesterRsrc",
                "--keepParent",
                &upgraded_app.to_string_lossy(),
                &zip_path.to_string_lossy(),
            ])
            .status()
            .unwrap()
            .success()
    } else {
        Command::new("zip")
            .current_dir(&build_dir)
            .args(["-qr", &zip_path.to_string_lossy(), "Fabushi.app"])
            .status()
            .unwrap()
            .success()
    };
    assert!(zip_ok, "archive creation for e2e test must succeed");
    let bytes = fs::read(&zip_path).unwrap();
    let sha = sha256_hex(&zip_path);
    (zip_path, bytes, sha)
}

async fn read_http_request_head(sock: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 512];
    loop {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn extract_range_start(req: &str) -> Option<usize> {
    for line in req.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("range:") {
            let trimmed = rest.trim();
            if let Some(bytes_part) = trimmed.strip_prefix("bytes=") {
                if let Some((start_s, _)) = bytes_part.split_once('-') {
                    return start_s.trim().parse::<usize>().ok();
                }
            }
        }
    }
    None
}

#[test]
fn test_semver_comparison_and_prerelease_rules() {
    assert_eq!(compare_semver("1.2.1", "1.2.0"), Ordering::Greater);
    assert_eq!(compare_semver("v1.3.0", "1.2.9"), Ordering::Greater);
    assert_eq!(
        compare_semver("macos-main-v1.2.78", "1.2.77"),
        Ordering::Greater
    );
    assert_eq!(compare_semver("1.2.0", "1.2.0-beta.1"), Ordering::Greater);
    assert_eq!(compare_semver("1.2.0", "v1.2.0"), Ordering::Equal);

    assert!(is_newer_version("1.2.0", "1.2.1"));
    assert!(is_newer_version("1.2.0-rc.1", "1.2.0"));
    assert!(is_newer_version("1.2.0", "v2.0.0"));
    assert!(!is_newer_version("1.2.0", "1.2.0"));
    assert!(!is_newer_version("1.2.0", "1.1.9"));
    assert!(!is_newer_version("1.2.0", "1.2.0-beta.2"));
    assert!(!is_newer_version("1.2.0", ""));
}

#[test]
fn test_parse_manifest_and_github_release_fallback_json() {
    let manifest_json = r#"{
        "version": "1.3.0",
        "tag_name": "v1.3.0",
        "release_notes": "Auto-update support",
        "platforms": {
            "darwin-arm64": {
                "zip_url": "https://github.com/bhrumom/gpui-telegram/releases/download/v1.3.0/Fabushi-1.3.0-macos-arm64.zip",
                "zip_sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "zip_size": 12345678,
                "dmg_url": "https://github.com/bhrumom/gpui-telegram/releases/download/v1.3.0/Fabushi-1.3.0-macos-arm64.dmg",
                "pkg_url": "https://github.com/bhrumom/gpui-telegram/releases/download/v1.3.0/Fabushi-1.3.0-macos-arm64.pkg"
            },
            "darwin-x86_64": {
                "zip_url": "https://github.com/bhrumom/gpui-telegram/releases/download/v1.3.0/Fabushi-1.3.0-macos-x86_64.zip",
                "zip_sha256": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB",
                "zip_size": 12345678
            }
        }
    }"#;
    let parsed = parse_release_manifest(manifest_json).expect("manifest should parse");
    assert_eq!(parsed.version, "1.3.0");
    assert!(parsed.asset_url.contains("Fabushi-1.3.0-macos-"));
    assert!(parsed.sha256.as_ref().is_some_and(|h| h.len() == 64));

    let gh_json = r#"{
        "tag_name": "v1.4.0",
        "name": "Fabushi v1.4.0",
        "body": "Release notes body",
        "assets": [
            {
                "name": "gpui_telegram-v1.4.0-linux-x86_64.tar.gz",
                "browser_download_url": "https://example.com/linux.tar.gz",
                "size": 100
            },
            {
                "name": "Fabushi-1.4.0-macos-arm64.dmg",
                "browser_download_url": "https://example.com/Fabushi-1.4.0-macos-arm64.dmg",
                "size": 200
            },
            {
                "name": "Fabushi-1.4.0-macos-arm64.pkg",
                "browser_download_url": "https://example.com/Fabushi-1.4.0-macos-arm64.pkg",
                "size": 210
            },
            {
                "name": "Fabushi-1.4.0-macos-arm64.zip",
                "browser_download_url": "https://example.com/Fabushi-1.4.0-macos-arm64.zip",
                "size": 300,
                "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            },
            {
                "name": "Fabushi-1.4.0-macos-x86_64.zip",
                "browser_download_url": "https://example.com/Fabushi-1.4.0-macos-x86_64.zip",
                "size": 305,
                "digest": "sha256:fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210"
            }
        ]
    }"#;
    let gh_parsed = parse_github_release_json(gh_json).expect("GitHub release JSON should parse");
    assert_eq!(gh_parsed.version, "1.4.0");
    assert!(gh_parsed.asset_url.ends_with(".zip"));
    assert!(gh_parsed.sha256.is_some());
    assert!(gh_parsed.dmg_url.as_deref().is_some_and(|u| u.ends_with(".dmg")));
    assert!(gh_parsed.pkg_url.as_deref().is_some_and(|u| u.ends_with(".pkg")));
}

#[test]
fn test_end_to_end_check_sha256_rejection_and_atomic_bundle_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let apps_dir = tmp.path().join("Applications");
    fs::create_dir_all(&apps_dir).unwrap();
    let installed_app = create_mock_app_bundle(&apps_dir, "1.2.0");

    let resolved = DesktopUpdater::resolve_target_app_bundle(
        &installed_app.join("Contents/MacOS/gpui_telegram_desktop"),
    );
    assert_eq!(resolved, installed_app);

    let build_dir = tmp.path().join("build_v130");
    let upgraded_app = create_mock_app_bundle(&build_dir, "1.3.0");
    let zip_path = tmp.path().join("Fabushi-1.3.0-macos-arm64.zip");

    let zip_ok = if Path::new("/usr/bin/ditto").exists() {
        Command::new("/usr/bin/ditto")
            .args([
                "-c",
                "-k",
                "--sequesterRsrc",
                "--keepParent",
                &upgraded_app.to_string_lossy(),
                &zip_path.to_string_lossy(),
            ])
            .status()
            .unwrap()
            .success()
    } else {
        Command::new("zip")
            .current_dir(&build_dir)
            .args(["-qr", &zip_path.to_string_lossy(), "Fabushi.app"])
            .status()
            .unwrap()
            .success()
    };
    assert!(zip_ok, "archive creation for e2e test must succeed");

    let real_sha = sha256_hex(&zip_path);
    let zip_url = format!("file://{}", zip_path.display());

    let bad_manifest_path = tmp.path().join("bad-latest-mac.json");
    let bad_manifest = serde_json::json!({
        "version": "1.3.0",
        "tag_name": "v1.3.0",
        "platforms": {
            "darwin-universal": {
                "zip_url": zip_url,
                "zip_sha256": "0000000000000000000000000000000000000000000000000000000000000000"
            }
        }
    });
    fs::write(&bad_manifest_path, serde_json::to_string(&bad_manifest).unwrap()).unwrap();

    let good_manifest_path = tmp.path().join("latest-mac.json");
    let good_manifest = serde_json::json!({
        "version": "1.3.0",
        "tag_name": "v1.3.0",
        "platforms": {
            "darwin-universal": {
                "zip_url": zip_url,
                "zip_sha256": real_sha,
                "zip_size": fs::metadata(&zip_path).unwrap().len()
            }
        }
    });
    fs::write(&good_manifest_path, serde_json::to_string(&good_manifest).unwrap()).unwrap();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let bad_feed = format!("file://{}", bad_manifest_path.display());
        let bad_info = DesktopUpdater::check_for_updates(Some(&bad_feed), "1.2.0")
            .await
            .unwrap()
            .expect("should detect 1.3.0");
        let staging_bad = tmp.path().join("staging_bad");
        let bad_res = DesktopUpdater::download_and_stage_update(&bad_info, &staging_bad, |_, _, _| {}).await;
        assert!(bad_res.is_err(), "tampered SHA-256 must fail verification");
        assert!(bad_res.unwrap_err().contains("SHA-256 verification failed"));

        let plist_still_old = fs::read_to_string(installed_app.join("Contents/Info.plist")).unwrap();
        assert!(plist_still_old.contains("<string>1.2.0</string>"));

        let good_feed = format!("file://{}", good_manifest_path.display());
        let up_to_date = DesktopUpdater::check_for_updates(Some(&good_feed), "1.3.0")
            .await
            .unwrap();
        assert!(up_to_date.is_none());

        let good_info = DesktopUpdater::check_for_updates(Some(&good_feed), "1.2.0")
            .await
            .unwrap()
            .expect("should detect newer 1.3.0");
        let staging_good = tmp.path().join("staging_good");
        let mut saw_progress = false;
        let staged_app = DesktopUpdater::download_and_stage_update(
            &good_info,
            &staging_good,
            |_, _, pct| {
                if pct == 100 {
                    saw_progress = true;
                }
            },
        )
        .await
        .expect("valid archive should stage cleanly");
        assert!(saw_progress);

        DesktopUpdater::replace_app_bundle_in_place(&staged_app, &installed_app)
            .expect("atomic replacement should succeed");

        let plist_new = fs::read_to_string(installed_app.join("Contents/Info.plist")).unwrap();
        assert!(plist_new.contains("<string>1.3.0</string>"));
        let out = Command::new(installed_app.join("Contents/MacOS/gpui_telegram_desktop"))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "Fabushi Desktop v1.3.0");
    });
}

#[test]
fn test_settings_advanced_and_about_fabushi_integration() {
    let about_legacy = AboutBox::new();
    assert!(about_legacy.version_text.contains("5.8.1"));

    let about_fabushi = AboutBox::new_fabushi();
    assert!(about_fabushi.version_text.contains(CURRENT_APP_VERSION));
    assert_eq!(about_fabushi.github_url, "https://github.com/bhrumom/gpui-telegram");

    let mut adv = SettingsAdvancedSections::new();
    assert_eq!(adv.current_version, CURRENT_APP_VERSION);
    assert!(adv.auto_update_enabled);
    assert_eq!(adv.download_path, "~/Downloads/Fabushi");

    let avail = DesktopUpdateState::Available {
        info: gpui_telegram_desktop::app::AvailableUpdateInfo {
            version: "1.9.0".to_string(),
            asset_url: "https://example.com/Fabushi-1.9.0-macos-arm64.zip".to_string(),
            sha256: None,
            size_bytes: None,
            release_notes: None,
            dmg_url: None,
            pkg_url: None,
        },
    };
    let snap = avail.to_snapshot(true);
    adv.sync_update_snapshot(&snap);
    assert_eq!(adv.latest_version.as_deref(), Some("1.9.0"));
    assert!(adv.update_available);
}

#[test]
fn test_resumable_http_download_recovers_from_midstream_drops_via_206_range() {
    let tmp = tempfile::tempdir().unwrap();
    let (_zip_path, bytes, sha) = create_mock_app_zip(tmp.path(), "1.3.0");
    let total = bytes.len();
    assert!(total > 300);

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let observed_ranges = Arc::new(Mutex::new(Vec::<Option<usize>>::new()));
        let observed_clone = observed_ranges.clone();
        let bytes_srv = bytes.clone();

        let srv = tokio::spawn(async move {
            let cut1 = total / 3;
            let cut2 = (total * 2) / 3;
            for req_idx in 0..3 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let req = read_http_request_head(&mut sock).await;
                let range_start = extract_range_start(&req);
                observed_clone.lock().unwrap().push(range_start);
                match req_idx {
                    0 => {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(&bytes_srv[..cut1]).await;
                        let _ = sock.shutdown().await;
                    }
                    1 => {
                        let start = range_start.expect("second request must send Range header");
                        let rem = total - start;
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {rem}\r\nContent-Range: bytes {start}-{}/{total}\r\nConnection: close\r\n\r\n",
                            total - 1
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(&bytes_srv[start..cut2]).await;
                        let _ = sock.shutdown().await;
                    }
                    _ => {
                        let start = range_start.expect("third request must send Range header");
                        let rem = total - start;
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {rem}\r\nContent-Range: bytes {start}-{}/{total}\r\nConnection: close\r\n\r\n",
                            total - 1
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(&bytes_srv[start..]).await;
                        let _ = sock.shutdown().await;
                    }
                }
            }
        });

        let info = AvailableUpdateInfo {
            version: "1.3.0".to_string(),
            asset_url: format!("http://{addr}/Fabushi-1.3.0-macos-arm64.zip"),
            sha256: Some(sha),
            size_bytes: Some(total as u64),
            release_notes: None,
            dmg_url: None,
            pkg_url: None,
        };
        let opts = UpdaterNetworkOptions {
            chunk_idle_timeout: Some(Duration::from_secs(2)),
            max_stall_retries: Some(5),
            ..Default::default()
        };
        let mut saw_100 = false;
        let staged = DesktopUpdater::download_and_stage_update_with_options(
            &info,
            &tmp.path().join("updates"),
            &opts,
            |_, _, pct| {
                if pct == 100 {
                    saw_100 = true;
                }
            },
        )
        .await
        .expect("resumable download across two midstream drops must succeed");

        srv.await.unwrap();
        assert!(saw_100);
        assert!(staged.join("Contents/Info.plist").is_file());
        let plist = fs::read_to_string(staged.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<string>1.3.0</string>"));
        let ranges = observed_ranges.lock().unwrap().clone();
        assert_eq!(ranges, vec![None, Some(total / 3), Some((total * 2) / 3)]);
    });
}

#[test]
fn test_chunk_idle_timeout_triggers_stream_recovery_without_decoding_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (_zip_path, bytes, sha) = create_mock_app_zip(tmp.path(), "1.3.0");
    let total = bytes.len();
    let half = total / 2;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bytes_srv = bytes.clone();

        let srv = tokio::spawn(async move {
            // Request 1: send first half then stall silently past chunk_idle_timeout.
            let (mut sock1, _) = listener.accept().await.unwrap();
            let req1 = read_http_request_head(&mut sock1).await;
            assert!(extract_range_start(&req1).is_none());
            let hdr1 = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            );
            let _ = sock1.write_all(hdr1.as_bytes()).await;
            let _ = sock1.write_all(&bytes_srv[..half]).await;
            let _ = sock1.flush().await;

            // Request 2: resume from `half` via 206 Partial Content while sock1 remains stalled.
            let (mut sock2, _) = listener.accept().await.unwrap();
            let req2 = read_http_request_head(&mut sock2).await;
            let start = extract_range_start(&req2).expect("resumed request must carry Range header");
            assert_eq!(start, half);
            let rem = total - start;
            let hdr2 = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {rem}\r\nContent-Range: bytes {start}-{}/{total}\r\nConnection: close\r\n\r\n",
                total - 1
            );
            let _ = sock2.write_all(hdr2.as_bytes()).await;
            let _ = sock2.write_all(&bytes_srv[start..]).await;
            let _ = sock2.shutdown().await;
            drop(sock1);
        });

        let info = AvailableUpdateInfo {
            version: "1.3.0".to_string(),
            asset_url: format!("http://{addr}/Fabushi-1.3.0-macos-arm64.zip"),
            sha256: Some(sha),
            size_bytes: Some(total as u64),
            release_notes: None,
            dmg_url: None,
            pkg_url: None,
        };
        let opts = UpdaterNetworkOptions {
            chunk_idle_timeout: Some(Duration::from_millis(150)),
            max_stall_retries: Some(3),
            ..Default::default()
        };
        let staged = DesktopUpdater::download_and_stage_update_with_options(
            &info,
            &tmp.path().join("updates"),
            &opts,
            |_, _, _| {},
        )
        .await
        .expect("idle timeout recovery must resume via Range and succeed");

        srv.await.unwrap();
        assert!(staged.join("Contents/Info.plist").is_file());
    });
}

#[test]
fn test_manual_retry_preserves_part_file_and_resumes_from_offset() {
    let tmp = tempfile::tempdir().unwrap();
    let (_zip_path, bytes, sha) = create_mock_app_zip(tmp.path(), "1.3.0");
    let total = bytes.len();
    let half = total / 2;

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bytes_srv = bytes.clone();
        let retry_range_seen = Arc::new(Mutex::new(None::<usize>));
        let retry_range_clone = retry_range_seen.clone();

        let srv = tokio::spawn(async move {
            // Attempt 1 (will fail due to max_stall_retries = 1 after early EOF at `half`).
            let (mut sock1, _) = listener.accept().await.unwrap();
            let req1 = read_http_request_head(&mut sock1).await;
            assert!(extract_range_start(&req1).is_none());
            let hdr1 = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
            );
            let _ = sock1.write_all(hdr1.as_bytes()).await;
            let _ = sock1.write_all(&bytes_srv[..half]).await;
            let _ = sock1.shutdown().await;

            // Attempt 2 (manual retry by user): must resume from `half`.
            let (mut sock2, _) = listener.accept().await.unwrap();
            let req2 = read_http_request_head(&mut sock2).await;
            let start = extract_range_start(&req2).expect("manual retry must send Range header");
            *retry_range_clone.lock().unwrap() = Some(start);
            let rem = total - start;
            let hdr2 = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {rem}\r\nContent-Range: bytes {start}-{}/{total}\r\nConnection: close\r\n\r\n",
                total - 1
            );
            let _ = sock2.write_all(hdr2.as_bytes()).await;
            let _ = sock2.write_all(&bytes_srv[start..]).await;
            let _ = sock2.shutdown().await;
        });

        let staging_root = tmp.path().join("updates");
        let info = AvailableUpdateInfo {
            version: "1.3.0".to_string(),
            asset_url: format!("http://{addr}/Fabushi-1.3.0-macos-arm64.zip"),
            sha256: Some(sha),
            size_bytes: Some(total as u64),
            release_notes: None,
            dmg_url: None,
            pkg_url: None,
        };
        let opts = UpdaterNetworkOptions {
            chunk_idle_timeout: Some(Duration::from_millis(500)),
            max_stall_retries: Some(1),
            ..Default::default()
        };

        let first_res =
            DesktopUpdater::download_and_stage_update_with_options(&info, &staging_root, &opts, |_, _, _| {})
                .await;
        assert!(first_res.is_err(), "first run with max_stall_retries=1 should stop after early EOF");

        let part_file = staging_root.join("stage-1.3.0/update.zip.part");
        assert!(part_file.is_file(), ".part file must remain on disk for manual retry");
        assert_eq!(fs::metadata(&part_file).unwrap().len(), half as u64);

        let second_res =
            DesktopUpdater::download_and_stage_update_with_options(&info, &staging_root, &opts, |_, _, _| {})
                .await
                .expect("manual retry must resume from preserved .part offset and succeed");
        srv.await.unwrap();

        assert_eq!(*retry_range_seen.lock().unwrap(), Some(half));
        assert!(second_res.join("Contents/Info.plist").is_file());
    });
}

#[test]
fn test_direct_download_bypasses_backend_relay() {
    let tmp = tempfile::tempdir().unwrap();
    let (_zip_path, bytes, sha) = create_mock_app_zip(tmp.path(), "1.3.0");
    let total = bytes.len();

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let primary_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let primary_addr = primary_listener.local_addr().unwrap();

        let bytes_srv = bytes.clone();
        let primary_srv = tokio::spawn(async move {
            let (mut sock, _) = primary_listener.accept().await.unwrap();
            let _req = read_http_request_head(&mut sock).await;
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n"
            );
            let _ = sock.write_all(hdr.as_bytes()).await;
            let _ = sock.write_all(&bytes_srv).await;
            let _ = sock.shutdown().await;
        });

        let info = AvailableUpdateInfo {
            version: "1.3.0".to_string(),
            asset_url: format!(
                "http://{primary_addr}/releases/download/v1.3.0/Fabushi-1.3.0-macos-arm64.zip"
            ),
            sha256: Some(sha),
            size_bytes: Some(total as u64),
            release_notes: None,
            dmg_url: None,
            pkg_url: None,
        };
        let opts = UpdaterNetworkOptions {
            // Even if server_url is populated, the updater must NOT use backend relay.
            server_url: Some("http://127.0.0.1:9".to_string()),
            release_download_base_override: Some(format!(
                "http://{primary_addr}/releases/download/"
            )),
            max_stall_retries: Some(1),
            ..Default::default()
        };

        let staged = DesktopUpdater::download_and_stage_update_with_options(
            &info,
            &tmp.path().join("updates"),
            &opts,
            |_, _, _| {},
        )
        .await
        .expect("updater must download directly from primary asset_url without contacting relay");

        primary_srv.await.unwrap();
        assert!(staged.join("Contents/Info.plist").is_file());
    });
}
