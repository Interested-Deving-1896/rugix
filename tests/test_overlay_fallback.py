"""The root overlay falls back to memory when its persistent storage is unusable."""

from __future__ import annotations

import pytest
from rugix_testkit import VMHandle


@pytest.fixture
def boot_system() -> str:
    return "customized-overlay-fallback"


def test_overlay_falls_back_to_in_memory(vm: VMHandle) -> None:
    """The system boots with an in-memory overlay, logs why, and reports the
    error state."""
    vm.run(["test", "-d", "/run/rugix/overlay/upper"], hide=True)

    log = vm.run(
        ["cat", "/run/rugix/state/.rugix/overlay-fallback-error.log"],
        hide=True,
    ).stdout
    assert "unable to setup system overlay mounts" in log

    info = vm.run_json(["rugix-ctrl", "system", "info", "--json"])
    assert info["state"]["status"] == "Error"
