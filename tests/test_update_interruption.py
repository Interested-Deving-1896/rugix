"""A forced reboot during an update leaves the active system selected."""

from __future__ import annotations

import shlex
from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

from support import BASE_PAGE, UPDATE_PAGE, web_page

# Rugix Ctrl has written this much to the target system slot when the VM reboots.
WRITTEN_BYTES = 64 * 1024 * 1024


# TODO: The SSH command stalls when `reboot -f` takes the VM down, so the test
# times out before it checks the rebooted system.
@pytest.mark.skip(reason="SSH session stalls on the forced reboot")
def test_forced_reboot_during_update_keeps_active_system(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """Rebooting while the target system slot is partially written boots the
    active system again. Installing the update again then succeeds."""
    bundle = bundle_url(bakery.bake_bundle("customized-amd64-delta"))

    # The rate limit keeps the installation running long enough to reboot
    # while Rugix Ctrl writes the system slot, the second payload.
    command = f"""
set -eu
curl --fail --silent --show-error --limit-rate 4m {shlex.quote(bundle)} \\
  </dev/null 2>/tmp/update.log |
  rugix-ctrl update install --reboot set --insecure-skip-bundle-verification - \\
  >>/tmp/update.log 2>&1 &
pid=$!
written() {{ awk '/^write_bytes:/ {{ print $2 }}' "/proc/$pid/io"; }}
start=
for _ in $(seq 1 1200); do
  kill -0 "$pid" 2>/dev/null || {{ cat /tmp/update.log >&2; exit 1; }}
  if [ -z "$start" ] && grep -q 'to slot system-b' /tmp/update.log; then
    start=$(written)
  fi
  if [ -n "$start" ] && [ $(($(written) - start)) -ge {WRITTEN_BYTES} ]; then
    reboot -f
    exit 0
  fi
  sleep 0.1
done
exit 1
"""
    vm.run_disconnecting(["sh", "-c", command], timeout=180)
    vm.wait_for_reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)

    rugix.install_and_reboot(bundle)
    rugix.assert_boot(default="a", active="b")
    assert UPDATE_PAGE in web_page(vm)
