"""Port of ``tests/tests/test-update-index-http.toml``.

Uses the in-VM nginx (installed by the ``hello-world`` recipe) to serve a
delta bundle over ``http://localhost``, exercising the HTTP fetch path of
``rugix-ctrl update install``. Reboots via ``rugix-ctrl system reboot
--spare`` after the install (the install itself uses ``--reboot no``).
"""

from __future__ import annotations

from rugix_testkit import BakeryBuilder, RugixCtrl, VMHandle

REMOTE_BUNDLE = "/var/www/html/system.rugixb"
INDEX_FILE = (
    "/run/rugix/mounts/data/rugix/slots/system-a/casync-64_sha512-256.rugix-block-index"
)


def test_update_index_http(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
) -> None:
    vm.run(
        ["rugix-ctrl", "slots", "create-index", "system-a", "casync-64", "sha512-256"],
        hide=True,
    )
    vm.run(["test", "-e", INDEX_FILE], hide=True)
    rugix.assert_boot(default="a", active="a")

    delta_base = bakery.bake_bundle("customized-amd64-delta")
    vm.upload(delta_base, REMOTE_BUNDLE)
    vm.run(["chmod", "777", REMOTE_BUNDLE], hide=True)

    rugix.update_install("http://localhost/system.rugixb", timeout=600)

    vm.run_disconnecting(["rugix-ctrl", "system", "reboot", "--spare"])
    vm.wait_for_reboot()

    rugix.assert_boot(default="a", active="b")
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
