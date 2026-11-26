"""Opaque-box 4-tier E2E test suite for Code Scene ("代码场景").

Pure Python 3 standard library only; zero third-party pip packages.
"""

SUITE_NAME = "code_scene_e2e"
SUITE_VERSION = "1.0.0"

EXPECTED_TIER_COUNTS = {
    1: 78,
    2: 79,
    3: 20,
    4: 11,
}
TOTAL_EXPECTED_TESTS = sum(EXPECTED_TIER_COUNTS.values())
TOTAL_MUTATIONS = 14
