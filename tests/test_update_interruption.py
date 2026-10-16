"""A forced reboot during an update leaves the active system selected."""

from __future__ import annotations

import shlex
from collections.abc import Callable
from pathlib import Path

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

from support import BASE_PAGE, UPDATE_PAGE, web_page

# Rugix Ctrl has written this much to the target slot when the VM reboots.
WRITTEN_BYTES = 64 * 1024 * 1024


def test_forced_reboot_during_update_keeps_active_system(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """Rebooting while the target slot is partially written boots the active
    system again. Installing the update again then succeeds."""
    bundle = bundle_url(bakery.bake_bundle("customized-amd64-delta"))

    # The rate limit keeps the installation running long enough to reboot
    # while Rugix Ctrl writes the target slot.
    command = f"""
set -eu
(curl --fail --silent --show-error --limit-rate 4m {shlex.quote(bundle)} |
  rugix-ctrl update install --reboot set --insecure-skip-bundle-verification -) \
  >/tmp/update.log 2>&1 &
for _ in $(seq 1 600); do
  pid=$(pgrep -n -f 'rugix-ctrl update install') || {{ cat /tmp/update.log >&2; exit 1; }}
  written=$(awk '/^write_bytes:/ {{ print $2 }}' "/proc/$pid/io")
  [ "$written" -ge {WRITTEN_BYTES} ] && reboot -f
  sleep 0.1
done
exit 1
"""
    vm.run_disconnecting(["sh", "-c", command], timeout=120)
    vm.wait_for_reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)

    rugix.install_and_reboot(bundle)
    rugix.assert_boot(default="a", active="b")
    assert UPDATE_PAGE in web_page(vm)
