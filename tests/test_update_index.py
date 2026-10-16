"""Updates retain block indices of the slots they write and compute missing
indices of source slots only when ``auto-index`` is enabled."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

INDEX_FILE = (
    "/run/rugix/mounts/data/rugix/slots/{slot}/casync-64_sha512-256.rugix-block-index"
)


@pytest.mark.parametrize("auto_index", [False, True], ids=["retain", "compute"])
def test_update_retains_target_index(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
    auto_index: bool,
) -> None:
    """Installing an update stores the target slot's index. The index of the
    active source slot is computed only with ``auto-index = true``."""
    source_index = INDEX_FILE.format(slot="system-a")
    target_index = INDEX_FILE.format(slot="system-b")
    if auto_index:
        vm.run(
            [
                "sh",
                "-c",
                "printf '[install]\\nauto-index = true\\n' >/etc/rugix/ctrl.toml",
            ],
            hide=True,
        )
    vm.run(["test", "!", "-e", source_index], hide=True)

    rugix.update_install(
        bundle_url(bakery.bake_bundle("customized-amd64")), timeout=600
    )

    vm.run(["test", "-e", target_index], hide=True)
    source_exists = vm.run(["test", "-e", source_index], check=False, hide=True).ok
    assert source_exists == auto_index
