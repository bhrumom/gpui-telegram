"""Shared pytest fixtures and CLI options for the Orca Pure-Rust E2E suite."""

from __future__ import annotations

from collections.abc import Callable, Generator
from pathlib import Path

import pytest

from harness import (
    CLI_BIN,
    SERVER_BIN,
    AuthSession,
    BotHomeFixture,
    GitRepoFixture,
    MockMcpUpstreamServer,
    MockOAuthServer,
    MockWebhookSink,
    OrcaApiClient,
    OrcaCliRunner,
    OrcaServerHandle,
    TempWorkspace,
)


def pytest_addoption(parser: pytest.Parser) -> None:
    group = parser.getgroup("orca-e2e", "Orca Pure-Rust E2E test options")
    group.addoption(
        "--server-url",
        action="store",
        default=None,
        help="Optional URL of an already-running gpui_telegram_server (overrides auto-spawn).",
    )
    group.addoption(
        "--server-bin",
        action="store",
        default=str(SERVER_BIN),
        help="Path to compiled gpui_telegram_server binary.",
    )
    group.addoption(
        "--cli-bin",
        action="store",
        default=str(CLI_BIN),
        help="Path to compiled fabushi CLI binary.",
    )


@pytest.fixture(scope="session")
def server_url_opt(request: pytest.FixtureRequest) -> str | None:
    val = request.config.getoption("--server-url")
    return str(val) if val else None


@pytest.fixture(scope="session")
def server_bin_path(request: pytest.FixtureRequest) -> Path:
    return Path(str(request.config.getoption("--server-bin")))


@pytest.fixture(scope="session")
def cli_bin_path(request: pytest.FixtureRequest) -> Path:
    return Path(str(request.config.getoption("--cli-bin")))


@pytest.fixture(scope="module")
def orca_server(
    server_url_opt: str | None,
    server_bin_path: Path,
) -> Generator[OrcaServerHandle, None, None]:
    """Module-scoped Orca server with isolated temp dir and ephemeral loopback port."""
    handle = OrcaServerHandle(server_bin=server_bin_path, external_url=server_url_opt)
    try:
        handle.start()
        yield handle
    finally:
        handle.stop(cleanup=True)


@pytest.fixture(scope="function")
def fresh_orca_server(
    server_url_opt: str | None,
    server_bin_path: Path,
) -> Generator[OrcaServerHandle, None, None]:
    """Function-scoped Orca server for restart, corruption, and stateful isolation tests."""
    handle = OrcaServerHandle(server_bin=server_bin_path, external_url=server_url_opt)
    try:
        handle.start()
        yield handle
    finally:
        handle.stop(cleanup=True)


@pytest.fixture(scope="function")
def auth_session(orca_server: OrcaServerHandle) -> AuthSession:
    """Freshly registered user session on the active orca_server."""
    return orca_server.register_user()


@pytest.fixture(scope="function")
def orca_api(orca_server: OrcaServerHandle, auth_session: AuthSession) -> OrcaApiClient:
    """Authenticated REST/RPC/WS client bound to orca_server and auth_session."""
    return orca_server.api_client(auth_session)


@pytest.fixture(scope="function")
def temp_workspace() -> Generator[TempWorkspace, None, None]:
    """Function-scoped RAII temporary workspace directory."""
    ws = TempWorkspace()
    try:
        yield ws
    finally:
        ws.cleanup()


@pytest.fixture(scope="function")
def orca_cli(
    cli_bin_path: Path,
    orca_server: OrcaServerHandle,
    auth_session: AuthSession,
    temp_workspace: TempWorkspace,
) -> OrcaCliRunner:
    """Isolated fabushi CLI runner bound to orca_server and auth_session."""
    return OrcaCliRunner(
        cli_bin=cli_bin_path,
        server_url=orca_server.base_url,
        token=auth_session.token,
        work_dir=temp_workspace.mkdir("cli_isolation"),
    )


@pytest.fixture(scope="function")
def git_repo() -> Generator[GitRepoFixture, None, None]:
    """Function-scoped initialized real Git repository with RAII cleanup."""
    repo = GitRepoFixture()
    try:
        yield repo
    finally:
        repo.cleanup()


@pytest.fixture(scope="function")
def bot_home_factory(
    temp_workspace: TempWorkspace,
) -> Callable[[str | None], BotHomeFixture]:
    """Factory creating isolated BotHomeFixture directories inside temp_workspace."""
    counter = 0

    def _create(folder_name: str | None = None) -> BotHomeFixture:
        nonlocal counter
        counter += 1
        name = folder_name or f"bot-home-{counter}"
        return BotHomeFixture(root=temp_workspace.mkdir(Path("bots") / name))

    return _create


@pytest.fixture(scope="function")
def mock_mcp_upstream() -> Generator[MockMcpUpstreamServer, None, None]:
    """Function-scoped external third-party MCP HTTP/SSE mock server."""
    srv = MockMcpUpstreamServer()
    try:
        yield srv
    finally:
        srv.stop()


@pytest.fixture(scope="function")
def mock_oauth_server() -> Generator[MockOAuthServer, None, None]:
    """Function-scoped external third-party OAuth 2.1 + PKCE + DCR mock server."""
    srv = MockOAuthServer()
    try:
        yield srv
    finally:
        srv.stop()


@pytest.fixture(scope="function")
def mock_webhook_sink() -> Generator[MockWebhookSink, None, None]:
    """Function-scoped external third-party webhook/Slack mirror receiver."""
    sink = MockWebhookSink()
    try:
        yield sink
    finally:
        sink.stop()
