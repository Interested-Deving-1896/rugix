"""Port of ``tests/tests/test-system-state.toml``.

Verifies persistence semantics for ``/var`` (ephemeral) vs.
``/run/rugix/state`` (persisted across reboot, cleared by factory reset).
Runs against ``customized-amd64`` and ``customized-temp-overlay``.
"""

from __future__ import annotations

import pytest
from rugix_testkit import VMHandle


@pytest.fixture(params=["customized-amd64", "customized-temp-overlay"])
def boot_system(request: pytest.FixtureRequest) -> str:
    return request.param


@pytest.mark.extended
def test_system_state_persistence(vm: VMHandle) -> None:
    vm.run(["touch", "/var/this-file-should-not-persist"], hide=True)
    vm.run(["touch", "/run/rugix/state/this-file-should-persist"], hide=True)

    vm.reboot()

    # /var was wiped, /run/rugix/state survived.
    res = vm.run(
        ["test", "-e", "/var/this-file-should-not-persist"], check=False, hide=True
    )
    assert not res.ok, "/var/this-file-should-not-persist must not survive reboot"

    vm.run(["test", "-e", "/run/rugix/state/this-file-should-persist"], hide=True)

    # Factory reset wipes /run/rugix/state too.
    vm.run_disconnecting(["rugix-ctrl", "state", "reset"])
    vm.wait_for_reboot()

    res = vm.run(
        ["test", "-e", "/run/rugix/state/this-file-should-persist"],
        check=False,
        hide=True,
    )
    assert not res.ok, "/run/rugix/state must be wiped after factory reset"
