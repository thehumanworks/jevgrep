import os
import shutil
import subprocess

import pytest


def _key_available() -> bool:
    if os.environ.get("TYPESAFE_API_KEY"):
        return True
    if not shutil.which("fnox"):
        return False
    try:
        return subprocess.run(["fnox", "get", "TYPESAFE_API_KEY"], capture_output=True, timeout=30).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def pytest_collection_modifyitems(config, items):
    if any("live" in item.keywords for item in items) and not _key_available():
        skip = pytest.mark.skip(reason="no TYPESAFE_API_KEY (env or fnox)")
        for item in items:
            if "live" in item.keywords:
                item.add_marker(skip)
