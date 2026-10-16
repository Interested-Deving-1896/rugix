"""Exercise slot-index retention and opt-in computation for system updates."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

SOURCE_INDEX_FILE = (
    "/run/rugix/mounts/data/rugix/slots/system-a/casync-64_sha512-256.rugix-block-index"
)
TARGET_INDEX_FILE = (
    "/run/rugix/mounts/data/rugix/slots/system-b/casync-64_sha512-256.rugix-block-index"
)


@pytest.fixture(params=[("system-a",), ("boot-a", "system-a")], ids=["single", "multi"])
def index_slots(request: pytest.FixtureRequest) -> tuple[str, ...]:
    return request.param


def test_update_index(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    index_slots: tuple[str, ...],
    bundle_url: Callable[[Path], str],
) -> None:
    for slot in index_slots:
        vm.run(
            ["rugix-ctrl", "slots", "create-index", slot, "casync-64", "sha512-256"],
            hide=True,
        )
    vm.run(["test", "-e", SOURCE_INDEX_FILE], hide=True)

    rugix.assert_boot(default="a", active="a")

    rugix.install_and_reboot(bundle_url(bakery.bake_bundle("customized-amd64")))

    rugix.assert_boot(default="a", active="b")
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")


@pytest.mark.parametrize(
    "compute_missing_indices", [False, True], ids=["retain", "compute"]
)
def test_update_indices(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
    compute_missing_indices: bool,
) -> None:
    """Always retain target indices; compute missing source indices only when enabled."""
    if compute_missing_indices:
        vm.run(
            [
                "sh",
                "-c",
                "printf '%s\\n' '[install]' 'auto-index = true' > /etc/rugix/ctrl.toml",
            ],
            hide=True,
        )
    vm.run(["test", "!", "-e", SOURCE_INDEX_FILE], hide=True)
    vm.run(["test", "!", "-e", TARGET_INDEX_FILE], hide=True)

    rugix.assert_boot(default="a", active="a")
    rugix.install_and_reboot(bundle_url(bakery.bake_bundle("customized-amd64")))

    source_check = ["test", "-e", SOURCE_INDEX_FILE]
    if not compute_missing_indices:
        source_check.insert(1, "!")
    vm.run(source_check, hide=True)
    vm.run(["test", "-e", TARGET_INDEX_FILE], hide=True)
    rugix.assert_boot(default="a", active="b")
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")

    rugix.install_and_reboot(bundle_url(bakery.bake_bundle("customized-amd64-delta")))
    vm.run(["test", "-e", SOURCE_INDEX_FILE], hide=True)
    vm.run(["test", "-e", TARGET_INDEX_FILE], hide=True)
    rugix.assert_boot(default="b", active="a")
