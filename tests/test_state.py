"""State management persists declared state and discards everything else.

The `persist-test-dir` recipe declares `/var/lib/rugix-test` as persistent and
installs `/var/lib/rugix-test/default` as a factory default.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

PERSISTED_DIR = "/var/lib/rugix-test"
BACKUP_NAME = "test-backup"


def test_state_persists_across_updates_until_reset(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """Declared directories keep their changes across an update while other
    writes are discarded. A factory reset with backup restores the defaults
    and keeps the previous state as a backup."""
    vm.run(
        [
            "sh",
            "-c",
            (
                f"echo changed >{PERSISTED_DIR}/default && touch {PERSISTED_DIR}/added "
                "/var/undeclared /run/rugix/state/app-file"
            ),
        ],
        hide=True,
    )

    rugix.install_and_reboot(bundle_url(bakery.bake_bundle("customized-amd64")))
    rugix.assert_boot(default="a", active="b")
    assert _read(vm, f"{PERSISTED_DIR}/default") == "changed"
    for path in (f"{PERSISTED_DIR}/added", "/run/rugix/state/app-file"):
        vm.run(["test", "-e", path], hide=True)
    assert not _exists(vm, "/var/undeclared")

    vm.run_disconnecting(
        ["rugix-ctrl", "state", "reset", "--backup", "--backup-name", BACKUP_NAME]
    )
    vm.wait_for_reboot()
    assert _read(vm, f"{PERSISTED_DIR}/default") == "factory"
    assert not _exists(vm, f"{PERSISTED_DIR}/added")
    assert not _exists(vm, "/run/rugix/state/app-file")
    backup = f"/run/rugix/mounts/data/state/{BACKUP_NAME}"
    vm.run(["test", "-e", f"{backup}/app-file"], hide=True)
    vm.run(["test", "-e", f"{backup}/persist{PERSISTED_DIR}/added"], hide=True)


def _read(vm: VMHandle, path: str) -> str:
    return vm.run(["cat", path], hide=True).stdout.strip()


def _exists(vm: VMHandle, path: str) -> bool:
    return vm.run(["test", "-e", path], check=False, hide=True).ok
