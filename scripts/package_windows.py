#!/usr/bin/env python3
"""
Automated Windows release packager for Fabushi (GPUI Telegram Desktop Client).

Produces in dist/:
1. Fabushi-{version}-windows-x86_64.zip        (portable archive with Fabushi.exe, gpui-tg-cli.exe, gpui_telegram_server.exe)
2. Fabushi-{version}-windows-x86_64-setup.exe  (1-click Inno Setup installer)
3. gpui-tg-cli-v{version}-windows-x86_64.exe   (standalone Action Bus CLI)
4. gpui_telegram_server-v{version}-windows-x86_64.exe (standalone server binary)
5. latest-windows.json                         (auto-updater manifest for Windows)
6. SHA256SUMS-windows.txt                      (SHA-256 checksums for all Windows artifacts)
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

GITHUB_REPO = "bhrumom/gpui-telegram"


def log(msg: str) -> None:
    print(f"[package_windows] {msg}", flush=True)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def read_workspace_version(repo_root: Path) -> str:
    cargo_toml = repo_root / "Cargo.toml"
    in_ws_pkg = False
    for line in cargo_toml.read_text(encoding="utf-8").splitlines():
        s = line.strip()
        if s == "[workspace.package]":
            in_ws_pkg = True
            continue
        if in_ws_pkg and s.startswith("["):
            break
        if in_ws_pkg and s.startswith("version"):
            _, val = s.split("=", 1)
            return val.strip().strip('"').strip("'")
    raise RuntimeError("Could not determine workspace version from Cargo.toml")


def find_iscc() -> str | None:
    found = shutil.which("iscc") or shutil.which("ISCC.exe")
    if found:
        return found
    candidates = [
        Path(r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe"),
        Path(r"C:\Program Files\Inno Setup 6\ISCC.exe"),
    ]
    for c in candidates:
        if c.is_file():
            return str(c)
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description="Package Fabushi Desktop Windows release.")
    parser.add_argument("--version", default="", help="Release version (defaults to Cargo.toml)")
    parser.add_argument("--arch", default="x86_64", help="Target architecture (default: x86_64)")
    parser.add_argument("--target-dir", default="target/release", help="Directory containing built .exe binaries")
    parser.add_argument("--dist-dir", default="dist", help="Output directory (default: dist)")
    parser.add_argument(
        "--require-installer",
        action="store_true",
        help="Fail if Inno Setup (ISCC.exe) is not found or fails",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    version = args.version.lstrip("v") if args.version else read_workspace_version(repo_root)
    tag_name = f"v{version}"
    arch = args.arch

    target_dir = (repo_root / args.target_dir).resolve()
    dist_dir = (repo_root / args.dist_dir).resolve()
    dist_dir.mkdir(parents=True, exist_ok=True)

    desktop_exe = target_dir / "gpui_telegram_desktop.exe"
    server_exe = target_dir / "gpui_telegram_server.exe"
    cli_exe = target_dir / "gpui-tg-cli.exe"

    if not desktop_exe.is_file():
        raise FileNotFoundError(f"Missing built desktop binary: {desktop_exe}")

    stage_dir = dist_dir / "windows-stage"
    if stage_dir.exists():
        shutil.rmtree(stage_dir)
    stage_dir.mkdir(parents=True, exist_ok=True)

    fabushi_exe = stage_dir / "Fabushi.exe"
    shutil.copy2(desktop_exe, fabushi_exe)
    log(f"Staged {fabushi_exe} ({fabushi_exe.stat().st_size:,} bytes)")

    staged_files = [fabushi_exe]
    if cli_exe.is_file():
        dst = stage_dir / "gpui-tg-cli.exe"
        shutil.copy2(cli_exe, dst)
        staged_files.append(dst)
        log(f"Staged {dst}")
    if server_exe.is_file():
        dst = stage_dir / "gpui_telegram_server.exe"
        shutil.copy2(server_exe, dst)
        staged_files.append(dst)
        log(f"Staged {dst}")

    readme_path = stage_dir / "README.txt"
    readme_path.write_text(
        f"Fabushi Desktop {tag_name} (Windows {arch})\r\n"
        f"=========================================\r\n\r\n"
        f"- Double-click Fabushi.exe to launch Fabushi Desktop.\r\n"
        f"- Releases & updates: https://github.com/{GITHUB_REPO}/releases\r\n",
        encoding="utf-8",
    )
    staged_files.append(readme_path)

    # 1. Portable ZIP archive
    zip_name = f"Fabushi-{version}-windows-{arch}.zip"
    zip_path = dist_dir / zip_name
    if zip_path.exists():
        zip_path.unlink()
    with zipfile.ZipFile(zip_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as zf:
        for fpath in staged_files:
            zf.write(fpath, arcname=f"Fabushi/{fpath.name}")
    zip_sha = sha256_file(zip_path)
    zip_size = zip_path.stat().st_size
    log(f"Created portable ZIP: {zip_path} ({zip_size:,} bytes, sha256={zip_sha[:12]}...)")

    # 2. Standalone CLI & server binaries for release upload
    artifacts: list[Path] = [zip_path]
    if cli_exe.is_file():
        cli_out = dist_dir / f"gpui-tg-cli-{tag_name}-windows-{arch}.exe"
        shutil.copy2(cli_exe, cli_out)
        artifacts.append(cli_out)
    if server_exe.is_file():
        srv_out = dist_dir / f"gpui_telegram_server-{tag_name}-windows-{arch}.exe"
        shutil.copy2(server_exe, srv_out)
        artifacts.append(srv_out)

    # 3. Inno Setup 1-click installer (.exe)
    setup_base = f"Fabushi-{version}-windows-{arch}-setup"
    setup_name = f"{setup_base}.exe"
    setup_path = dist_dir / setup_name
    iscc = find_iscc()
    iss_script = repo_root / "scripts" / "windows" / "FabushiSetup.iss"
    icon_path = repo_root / "crates" / "gpui_telegram_desktop" / "assets" / "AppIcon.ico"

    if iscc and iss_script.is_file():
        cmd = [
            iscc,
            f"/DAppVersion={version}",
            f"/DStageDir={stage_dir}",
            f"/DOutDir={dist_dir}",
            f"/DOutBaseFilename={setup_base}",
            f"/DIconPath={icon_path}",
            str(iss_script),
        ]
        log(f"Running Inno Setup: {' '.join(cmd)}")
        subprocess.run(cmd, check=True)
        if not setup_path.is_file():
            raise RuntimeError(f"Inno Setup did not produce expected installer: {setup_path}")
        setup_sha = sha256_file(setup_path)
        log(f"Created installer: {setup_path} ({setup_path.stat().st_size:,} bytes, sha256={setup_sha[:12]}...)")
        artifacts.insert(0, setup_path)
    elif args.require_installer:
        raise RuntimeError("Inno Setup (ISCC.exe) not found, cannot build Windows installer.")
    else:
        log("Inno Setup (ISCC.exe) not found; skipping .exe installer creation.")

    # 4. latest-windows.json auto-updater manifest
    base_url = f"https://github.com/{GITHUB_REPO}/releases/download/{tag_name}"
    entry = {
        "zip_url": f"{base_url}/{zip_name}",
        "zip_sha256": zip_sha,
        "zip_size": zip_size,
        "pkg_url": f"{base_url}/{setup_name}" if setup_path.is_file() else None,
    }
    manifest = {
        "version": version,
        "tag_name": tag_name,
        "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "release_notes": f"Fabushi Desktop {tag_name} (Windows {arch})",
        "platforms": {
            f"windows-{arch}": entry,
            "windows-x64": entry,
            "win32-x64": entry,
        },
    }
    manifest_path = dist_dir / "latest-windows.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    log(f"Generated auto-updater manifest: {manifest_path}")

    # 5. SHA256SUMS-windows.txt
    sums_lines = [f"{sha256_file(a)}  {a.name}" for a in artifacts]
    sums_path = dist_dir / "SHA256SUMS-windows.txt"
    sums_path.write_text("\n".join(sums_lines) + "\n", encoding="utf-8")
    log(f"Generated checksums: {sums_path}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
