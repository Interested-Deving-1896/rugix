"""Port of ``tests/tests/test-update-static-delta.toml``.

Three-step update: first installs the regular ``customized-amd64`` bundle,
then installs a static delta to ``customized-amd64-delta``. A later regular
update computes the missing source index only when enabled.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, Bundler, RugixCtrl, VMHandle

SYSTEM_INDEX_FILE = (
    "/run/rugix/mounts/data/rugix/slots/{slot}/casync-64_sha512-256.rugix-block-index"
)


@pytest.fixture(scope="session")
def delta_bundle(bakery: BakeryBuilder, bundler: Bundler, rugix_project: Path) -> Path:
    base = bakery.bake_bundle("customized-amd64")
    target = bakery.bake_bundle("customized-amd64-delta")
    output = rugix_project / "build" / "delta.rugixb"
    bundler.delta(base, target, output)
    return output


@pytest.mark.parametrize(
    "compute_missing_indices", [False, True], ids=["retain", "compute"]
)
def test_update_static_delta(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    delta_bundle: Path,
    bundle_url: Callable[[Path], str],
    compute_missing_indices: bool,
) -> None:
    """Static deltas leave no target index; later installs may compute it as a source."""
    base = bundle_url(bakery.bake_bundle("customized-amd64"))
    delta = bundle_url(delta_bundle)
    source_index = SYSTEM_INDEX_FILE.format(slot="system-b")
    target_index = SYSTEM_INDEX_FILE.format(slot="system-a")

    rugix.assert_boot(default="a", active="a")

    rugix.install_and_reboot(base)
    rugix.assert_boot(default="a", active="b")
    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")

    vm.run(["test", "-e", source_index], hide=True)
    for update in (delta, base):
        if compute_missing_indices:
            vm.run(
                [
                    "sh",
                    "-c",
                    "printf '%s\\n' '[install]' 'auto-index = true' > /etc/rugix/ctrl.toml",
                ],
                hide=True,
            )
        rugix.install_and_reboot(update)
        if update == delta:
            vm.run(["test", "!", "-e", target_index], hide=True)
            rugix.assert_boot(default="b", active="a")
            rugix.system_commit()
            rugix.assert_boot(default="a", active="a")
        else:
            target_check = ["test", "-e", target_index]
            if not compute_missing_indices:
                target_check.insert(1, "!")
            vm.run(target_check, hide=True)
            vm.run(["test", "-e", source_index], hide=True)
            rugix.assert_boot(default="a", active="b")
