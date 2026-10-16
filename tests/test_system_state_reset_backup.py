"""Port of ``tests/tests/test-system-state-reset-backup.toml``.

Same persistence semantics as ``test_system_state``, but the factory
reset is invoked with ``--backup``: state should land under
``/run/rugix/mounts/data/state/<backup-name>/`` rather than be deleted.
"""

from __future__ import annotations

import pytest
from rugix_testkit import VMHandle

BACKUP_NAME = "test-backup"


@pytest.mark.extended
def test_system_state_reset_with_backup(vm: VMHandle) -> None:
    vm.run(["touch", "/var/this-file-should-not-persist"], hide=True)
    vm.run(["touch", "/run/rugix/state/this-file-should-persist"], hide=True)

    vm.reboot()

    res = vm.run(
        ["test", "-e", "/var/this-file-should-not-persist"], check=False, hide=True
    )
    assert not res.ok
    vm.run(["test", "-e", "/run/rugix/state/this-file-should-persist"], hide=True)

    vm.run_disconnecting(
        [
            "rugix-ctrl",
            "state",
            "reset",
            "--backup",
            "--backup-name",
            BACKUP_NAME,
        ]
    )
    vm.wait_for_reboot()

    res = vm.run(
        ["test", "-e", "/run/rugix/state/this-file-should-persist"],
        check=False,
        hide=True,
    )
    assert not res.ok, "/run/rugix/state must be wiped after factory reset"

    backup_path = f"/run/rugix/mounts/data/state/{BACKUP_NAME}/this-file-should-persist"
    vm.run(["test", "-e", backup_path], hide=True)
