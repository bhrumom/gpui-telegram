"""RAII filesystem, real Git repository, Bot Home, and SQLite test fixtures.

All fixtures clean up temporary directories deterministically (AGENTS.md §1.3).
Pure Python 3.12 stdlib + /usr/bin/git only.
"""

from __future__ import annotations

import json
import sqlite3
import subprocess
import tempfile
from pathlib import Path
from typing import Any


class TempWorkspace:
    """RAII wrapper around tempfile.TemporaryDirectory with convenient path helpers."""

    def __init__(self, prefix: str = "orca_e2e_ws_") -> None:
        self._tempdir = tempfile.TemporaryDirectory(prefix=prefix)
        self.path = Path(self._tempdir.name)

    def resolve(self, rel: str | Path) -> Path:
        return self.path / rel

    def mkdir(self, rel: str | Path) -> Path:
        target = self.resolve(rel)
        target.mkdir(parents=True, exist_ok=True)
        return target

    def write(self, rel: str | Path, content: str | bytes) -> Path:
        target = self.resolve(rel)
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8")
        return target

    def read(self, rel: str | Path) -> str:
        return self.resolve(rel).read_text(encoding="utf-8")

    def read_bytes(self, rel: str | Path) -> bytes:
        return self.resolve(rel).read_bytes()

    def exists(self, rel: str | Path) -> bool:
        return self.resolve(rel).exists()

    def cleanup(self) -> None:
        self._tempdir.cleanup()

    def __enter__(self) -> TempWorkspace:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.cleanup()


class GitRepoFixture:
    """Real local Git repository initialized via /usr/bin/git inside an RAII temp dir."""

    GIT_BIN = "/usr/bin/git"

    def __init__(self, root: Path | None = None, initial_branch: str = "main") -> None:
        self._owned_ws: TempWorkspace | None = None
        if root is None:
            self._owned_ws = TempWorkspace(prefix="orca_e2e_git_")
            self.path = self._owned_ws.path
        else:
            self.path = Path(root)
            self.path.mkdir(parents=True, exist_ok=True)

        self.git("init", "-b", initial_branch)
        self.git("config", "user.name", "Orca E2E")
        self.git("config", "user.email", "e2e@orca.local")
        self.git("config", "commit.gpgsign", "false")

    def git(self, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [self.GIT_BIN, *args],
            cwd=str(self.path),
            text=True,
            capture_output=True,
            check=check,
        )

    def write_file(self, rel: str | Path, content: str | bytes) -> Path:
        target = self.path / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            target.write_bytes(content)
        else:
            target.write_text(content, encoding="utf-8")
        return target

    def commit_file(self, rel: str | Path, content: str | bytes, msg: str = "commit") -> str:
        self.write_file(rel, content)
        self.git("add", str(rel))
        self.git("commit", "-m", msg)
        return self.git("rev-parse", "HEAD").stdout.strip()

    def create_ignored_file(
        self,
        rel: str | Path,
        content: str | bytes,
        ignore_pattern: str | None = None,
        commit_gitignore: bool = True,
    ) -> Path:
        pattern = ignore_pattern if ignore_pattern is not None else str(rel)
        gitignore = self.path / ".gitignore"
        existing = gitignore.read_text(encoding="utf-8").splitlines() if gitignore.exists() else []
        if pattern not in existing:
            existing.append(pattern)
            gitignore.write_text("\n".join(existing) + "\n", encoding="utf-8")
            if commit_gitignore:
                self.git("add", ".gitignore")
                self.git("commit", "-m", f"ignore {pattern}")
        return self.write_file(rel, content)

    def write_orca_yaml(self, content: str, commit: bool = True) -> Path:
        if commit:
            self.commit_file("orca.yaml", content, msg="add orca.yaml")
            return self.path / "orca.yaml"
        return self.write_file("orca.yaml", content)

    def write_worktreeinclude(self, lines: list[str], commit: bool = True) -> Path:
        text = "\n".join(lines) + ("\n" if lines else "")
        if commit:
            self.commit_file(".worktreeinclude", text, msg="add .worktreeinclude")
            return self.path / ".worktreeinclude"
        return self.write_file(".worktreeinclude", text)

    def create_branch(self, name: str, checkout: bool = False) -> str:
        if checkout:
            self.git("checkout", "-b", name)
        else:
            self.git("branch", name)
        return name

    def current_branch(self) -> str:
        return self.git("rev-parse", "--abbrev-ref", "HEAD").stdout.strip()

    def cleanup(self) -> None:
        if self._owned_ws is not None:
            self._owned_ws.cleanup()
            self._owned_ws = None

    def __enter__(self) -> GitRepoFixture:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.cleanup()


class BotHomeFixture:
    """Synthesizes Orca bot home folders and foreign OpenClaw/Hermes/OpenBot trees."""

    def __init__(self, root: Path | None = None) -> None:
        self._owned_ws: TempWorkspace | None = None
        if root is None:
            self._owned_ws = TempWorkspace(prefix="orca_e2e_bot_")
            self.root = self._owned_ws.path
        else:
            self.root = Path(root)
            self.root.mkdir(parents=True, exist_ok=True)

    def _write_text(self, rel: str | Path, content: str) -> Path:
        target = self.root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return target

    def write_soul(self, content: str) -> Path:
        return self._write_text("SOUL.md", content)

    def write_memory(self, content: str) -> Path:
        return self._write_text("MEMORY.md", content)

    def write_agents_md(self, content: str) -> Path:
        return self._write_text("AGENTS.md", content)

    def write_claude_md(self, content: str = "@AGENTS.md\n") -> Path:
        return self._write_text("CLAUDE.md", content)

    def write_bot_json(
        self,
        bot_id: str,
        *,
        name: str = "Test Bot",
        folder: str = "test-bot",
        agent: str = "claude",
        avatar_revision: int = 0,
        extra: dict[str, Any] | None = None,
    ) -> Path:
        payload: dict[str, Any] = {
            "id": bot_id,
            "name": name,
            "folder": folder,
            "agent": agent,
            "avatarRevision": avatar_revision,
        }
        if extra:
            payload.update(extra)
        return self._write_text("bot.json", json.dumps(payload, indent=2) + "\n")

    def write_topic_memory(self, topic: str, content: str) -> Path:
        return self._write_text(Path("memory") / f"{topic}.md", content)

    def write_journal(self, year_month: str, content: str) -> Path:
        return self._write_text(Path("memory") / "journal" / f"{year_month}.md", content)

    def write_skill(self, skill_name: str, content: str) -> Path:
        return self._write_text(Path("skills") / skill_name / "SKILL.md", content)

    def populate_default(
        self,
        bot_id: str = "bot-e2e-1",
        *,
        name: str = "Review Bot",
        folder: str = "review-bot",
        agent: str = "claude",
    ) -> BotHomeFixture:
        self.write_soul(
            f"# {name}\n\nReviews pull requests thoroughly and concisely.\n\n"
            "## How I work\n- Check tests before approving.\n"
        )
        self.write_memory(
            "# Memory\n\n"
            "- 2026-11-25 Rule: Never merge red CI.\n"
            "- 2026-11-25 Fact: Default branch is main.\n"
        )
        self.write_agents_md("# Generated Orca Bot Instructions\n")
        self.write_claude_md("@AGENTS.md\n")
        self.write_bot_json(bot_id, name=name, folder=folder, agent=agent)
        self.write_topic_memory("ci", "# CI Notes\n- Linux uses mold linker.\n")
        self.write_journal(
            "2026-11",
            "- 2026-11-25 | item #1 | done | PR #42 | Initial setup\n",
        )
        self.write_skill(
            "pr-review-checklist",
            "---\nname: pr-review-checklist\n---\nCheck edge cases and tests.\n",
        )
        return self

    @staticmethod
    def create_foreign_openclaw(
        base_dir: Path,
        *,
        bot_name: str = "ClawOps",
        identity_md: str | None = None,
        soul_md: str | None = None,
    ) -> Path:
        oc_dir = Path(base_dir) / ".openclaw"
        oc_dir.mkdir(parents=True, exist_ok=True)
        (oc_dir / "IDENTITY.md").write_text(
            identity_md or f"# {bot_name}\nOperations assistant migrated from OpenClaw.\n",
            encoding="utf-8",
        )
        (oc_dir / "SOUL.md").write_text(
            soul_md or "Calm, thorough, never leaks credentials.\n",
            encoding="utf-8",
        )
        return oc_dir

    @staticmethod
    def create_foreign_hermes(
        base_dir: Path,
        *,
        profile: str = "default",
        soul_md: str | None = None,
        config_yaml: str | None = None,
    ) -> Path:
        prof_dir = Path(base_dir) / "profiles" / profile
        prof_dir.mkdir(parents=True, exist_ok=True)
        (prof_dir / "SOUL.md").write_text(
            soul_md or f"# Hermes {profile}\nAssists with triage and incident summaries.\n",
            encoding="utf-8",
        )
        (prof_dir / "config.yaml").write_text(
            config_yaml or f"name: Hermes-{profile}\nmodel: claude-sonnet\n",
            encoding="utf-8",
        )
        return prof_dir

    @staticmethod
    def create_foreign_openbot(
        base_dir: Path,
        *,
        agent_name: str = "ReleaseHelper",
        soul_md: str | None = None,
    ) -> Path:
        agent_dir = Path(base_dir) / "OpenBot" / "Agents" / agent_name
        agent_dir.mkdir(parents=True, exist_ok=True)
        (agent_dir / "SOUL.md").write_text(
            soul_md or f"# {agent_name}\nPrepares release notes and verifies tags.\n",
            encoding="utf-8",
        )
        return agent_dir

    def cleanup(self) -> None:
        if self._owned_ws is not None:
            self._owned_ws.cleanup()
            self._owned_ws = None

    def __enter__(self) -> BotHomeFixture:
        return self

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> None:
        self.cleanup()


class SqliteFixtureHelper:
    """WAL-mode SQLite helper for PRAGMA user_version, trigger, and corruption testing."""

    def __init__(self, db_path: Path) -> None:
        self.db_path = Path(db_path)
        self.db_path.parent.mkdir(parents=True, exist_ok=True)
        conn = self.connect()
        try:
            conn.execute("PRAGMA journal_mode = WAL;")
            conn.execute("PRAGMA foreign_keys = ON;")
            conn.commit()
        finally:
            conn.close()

    def connect(self, read_only: bool = False) -> sqlite3.Connection:
        if read_only:
            return sqlite3.connect(f"file:{self.db_path}?mode=ro", uri=True)
        conn = sqlite3.connect(str(self.db_path))
        conn.execute("PRAGMA foreign_keys = ON;")
        return conn

    def get_user_version(self) -> int:
        conn = self.connect()
        try:
            row = conn.execute("PRAGMA user_version;").fetchone()
            return int(row[0]) if row else 0
        finally:
            conn.close()

    def set_user_version(self, version: int) -> None:
        if isinstance(version, bool) or not isinstance(version, int) or version < 0:
            raise ValueError(f"user_version must be non-negative int, got {version!r}")
        conn = self.connect()
        try:
            conn.execute(f"PRAGMA user_version = {version};")
            conn.commit()
        finally:
            conn.close()

    def exec_sql(
        self, sql: str, params: tuple[Any, ...] | dict[str, Any] = ()
    ) -> list[tuple[Any, ...]]:
        conn = self.connect()
        try:
            cur = conn.execute(sql, params)
            rows = cur.fetchall()
            conn.commit()
            return rows
        finally:
            conn.close()

    def exec_script(self, sql: str) -> None:
        conn = self.connect()
        try:
            conn.executescript(sql)
            conn.commit()
        finally:
            conn.close()

    @staticmethod
    def corrupt_header(db_path: Path, length: int = 100) -> None:
        target = Path(db_path)
        data = bytearray(target.read_bytes()) if target.exists() else bytearray(max(length, 4096))
        corrupt_prefix = (b"\xffORCA_E2E_CORRUPT_HEADER\x00" * ((length // 25) + 1))[:length]
        if len(data) < length:
            data.extend(b"\x00" * (length - len(data)))
        data[:length] = corrupt_prefix
        target.write_bytes(bytes(data))
