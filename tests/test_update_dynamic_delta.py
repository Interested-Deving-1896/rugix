"""Dynamic delta updates download only the blocks missing from the local slots.

The bundle is served by nginx on the VM, which supports HTTP range requests,
unlike the host's HTTP server used by other tests.
"""

from __future__ import annotations

import re

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

from support import UPDATE_PAGE, web_page

REMOTE_BUNDLE = "/var/www/html/system.rugixb"


def test_dynamic_delta_downloads_changed_blocks(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
) -> None:
    """Indexing the active slots lets an update fetch only changed blocks."""
    for slot in ("boot-a", "system-a"):
        vm.run(
            ["rugix-ctrl", "slots", "create-index", slot, "casync-64", "sha512-256"],
            hide=True,
        )
    vm.upload(bakery.bake_bundle("customized-amd64-delta"), REMOTE_BUNDLE)
    vm.run(["chmod", "644", REMOTE_BUNDLE], hide=True)

    result = rugix.update_install("http://localhost/system.rugixb", timeout=600)
    downloaded = re.search(r"downloaded ([\d.]+)% .* of the full bundle", result.stderr)
    assert downloaded is not None, result.stderr
    assert float(downloaded.group(1)) < 5

    vm.run_disconnecting(["rugix-ctrl", "system", "reboot", "--spare"])
    vm.wait_for_reboot()
    rugix.assert_boot(default="a", active="b")
    assert UPDATE_PAGE in web_page(vm)
