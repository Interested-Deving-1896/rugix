"""A forced reboot during streaming installation must not select an incomplete target."""

from __future__ import annotations

import shlex
from collections.abc import Callable
from pathlib import Path

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle


def test_forced_reboot_during_update_is_resumable(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    bundle = bundle_url(bakery.bake_bundle("customized-amd64"))
    rugix.assert_boot(default="a", active="a")

    # Rate limiting keeps the producer alive while Rugix is decoding the inactive
    # target. The explicit liveness check prevents a completed update from being
    # mistaken for an interrupted one.
    command = f"""
set -eu
(curl --fail --silent --show-error --limit-rate 1m {shlex.quote(bundle)} |
  rugix-ctrl update install --reboot set \
    --insecure-skip-bundle-verification -) >/tmp/interrupted-update.log 2>&1 &
installer=$!
sleep 8
kill -0 "$installer"
reboot -f
"""
    vm.run_disconnecting(["sh", "-c", command], timeout=30)
    vm.wait_for_reboot(timeout=600)

    rugix.assert_boot(default="a", active="a")
    assert not vm.run(
        ["test", "-e", "/run/rugix/mounts/data/.rugix/deferred-reboot-spare"],
        check=False,
        hide=True,
    ).ok

    # Reinstalling overwrites the inactive destination and completes normally.
    rugix.update_install(bundle, reboot="set", insecure=True, timeout=600)
    vm.reboot(timeout=600)
    rugix.assert_boot(default="a", active="b")
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
