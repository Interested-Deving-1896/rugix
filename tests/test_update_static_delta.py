"""Static delta bundles update a system from a slot with the delta's source.

Rugix Ctrl recognizes the source of a static delta only in slots that it
installed from a bundle, so the test first installs the full source bundle.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, Bundler, RugixCtrl, VMHandle

from support import UPDATE_PAGE, exists, web_page

TARGET_INDEX = (
    "/run/rugix/mounts/data/rugix/slots/system-a/casync-64_sha512-256.rugix-block-index"
)


@pytest.fixture(scope="module")
def delta_bundle(bakery: BakeryBuilder, bundler: Bundler, rugix_project: Path) -> Path:
    """Static delta from `customized-amd64` to a system with another web page."""
    base = bakery.bake_bundle("customized-amd64")
    target = bakery.bake_bundle("customized-amd64-delta")
    output = rugix_project / "build" / "delta.rugixb"
    bundler.delta(base, target, output)
    return output


def test_static_delta_installs_target_system(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    delta_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """The delta reconstructs the target system from the installed source and
    discards the stale block index of the overwritten slot."""
    rugix.install_and_reboot(bundle_url(bakery.bake_bundle("customized-amd64")))
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
    vm.run(
        ["rugix-ctrl", "slots", "create-index", "system-a", "casync-64", "sha512-256"],
        hide=True,
    )
    assert exists(vm, TARGET_INDEX)

    rugix.install_and_reboot(bundle_url(delta_bundle))

    rugix.assert_boot(default="b", active="a")
    assert UPDATE_PAGE in web_page(vm)
    # The index described the previous contents of the slot and static deltas
    # do not deliver a new one.
    assert not exists(vm, TARGET_INDEX)
