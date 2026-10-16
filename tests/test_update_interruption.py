"""A forced reboot during an update leaves the active system selected."""

from __future__ import annotations

import shlex
from collections.abc import Callable
from pathlib import Path

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

from support import BASE_PAGE, UPDATE_PAGE, web_page

# Rugix Ctrl has written this much to the target system slot when the VM reboots.
WRITTEN_BYTES = 64 * 1024 * 1024
REMOTE_SCRIPT = "/tmp/interrupt-update.sh"


def test_forced_reboot_during_update_keeps_active_system(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
    tmp_path: Path,
) -> None:
    """Rebooting while the target system slot is partially written boots the
    active system again. Installing the update again then succeeds."""
    bundle = bundle_url(bakery.bake_bundle("customized-amd64-delta"))

    # The rate limit keeps the installation running long enough to reboot
    # while Rugix Ctrl writes the system slot, the second payload. If the
    # installation ends early, the script writes its log to the serial console
    # and the test times out waiting for the reboot.
    script = tmp_path / "interrupt-update.sh"
    script.write_text(f"""\
set -eu
curl --fail --silent --show-error --limit-rate 4m {shlex.quote(bundle)} \\
  2>/tmp/update.log |
  rugix-ctrl update install --reboot set --insecure-skip-bundle-verification - \\
  >>/tmp/update.log 2>&1 &
pid=$!
written() {{ awk '/^write_bytes:/ {{ print $2 }}' "/proc/$pid/io"; }}
start=
while kill -0 "$pid" 2>/dev/null; do
  if [ -z "$start" ] && grep -q 'to slot system-b' /tmp/update.log; then
    start=$(written)
  fi
  if [ -n "$start" ] && [ $(($(written) - start)) -ge {WRITTEN_BYTES} ]; then
    exec reboot -f
  fi
  sleep 0.1
done
cat /tmp/update.log >/dev/console
""")
    vm.upload(script, REMOTE_SCRIPT)
    # A forced reboot can leave an SSH session hanging, so the script runs
    # detached from it.
    vm.run(
        ["sh", "-c", f"setsid -f sh {REMOTE_SCRIPT} </dev/null >/dev/null 2>&1"],
        hide=True,
    )
    vm.wait_for_reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)

    rugix.install_and_reboot(bundle)
    rugix.assert_boot(default="a", active="b")
    assert UPDATE_PAGE in web_page(vm)
