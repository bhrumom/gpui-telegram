"""OpenBot 1:1 UI/UX Comprehensive Refactor — 4-Tier Opaque-Box E2E Test Suite (`UI-F01`..`UI-F20`).

100% Python 3.10+ standard library. Zero internal Rust imports, zero external dependencies.
"""

from __future__ import annotations

from tests.e2e_openbot_uiux.harness import (
    ALL_FEATURES,
    PROBE_SCHEMA_VERSION,
    PROBE_SUITE_NAME,
    TEST_REGISTRY,
    TempUiUxEnv,
    UiUxE2ETestCase,
    UiUxRuntimeProbe,
    uiux_e2e_test,
)

__all__ = [
    "ALL_FEATURES",
    "PROBE_SCHEMA_VERSION",
    "PROBE_SUITE_NAME",
    "TEST_REGISTRY",
    "TempUiUxEnv",
    "UiUxE2ETestCase",
    "UiUxRuntimeProbe",
    "uiux_e2e_test",
]
