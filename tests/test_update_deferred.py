"""Deferred update selection survives a shutdown boundary and targets the recorded group."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle


def test_deferred_update_reboots_into_recorded_group(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    bundle = bundle_url(bakery.bake_bundle("customized-amd64"))
    rugix.assert_boot(default="a", active="a")

    result = rugix.update_install(
        bundle,
        reboot="deferred",
        insecure=True,
        timeout=600,
        check=False,
    )
    assert result.ok, f"{result.stdout}\n{result.stderr}"
    rugix.assert_boot(default="a", active="a")

    marker = vm.run(
        ["cat", "/run/rugix/mounts/data/.rugix/deferred-reboot-spare"],
        hide=True,
    )
    assert json.loads(marker.stdout) == {"version": 1, "bootGroup": "b"}

    # Rugix processes the durable marker before userspace services start, selects
    # group B, clears the marker, and performs the second reboot automatically.
    vm.reboot(timeout=600)
    rugix.assert_boot(default="a", active="b")
    assert not vm.run(
        ["test", "-e", "/run/rugix/mounts/data/.rugix/deferred-reboot-spare"],
        check=False,
        hide=True,
    ).ok

    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
