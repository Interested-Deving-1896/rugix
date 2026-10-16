"""Port of ``tests/tests/test-update-custom-slots.toml``.

Boots the ``customized-slots`` system, builds ``bundles/slots-bundle``
(tarring its payloads first), installs it, and verifies that the slot
payloads land at the expected paths under ``/run/rugix/state``. The
incremental update retains the file slot index without indexing custom slots.
"""

from __future__ import annotations

import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, RugixCtrl, VMHandle


@pytest.fixture
def boot_system() -> str:
    return "customized-slots"


@pytest.fixture(scope="session")
def slots_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    payloads = rugix_project / "bundles" / "slots-bundle" / "payloads"
    test_tar = payloads / "test-dir.tar"
    test_tar.unlink(missing_ok=True)
    subprocess.run(
        ["tar", "-cf", str(test_tar), "-C", str(payloads), "test-file"],
        check=True,
    )
    output = rugix_project / "build" / "slots-bundle.rugixb"
    if output.exists():
        output.unlink()
    bundler.bundle(rugix_project / "bundles" / "slots-bundle", output)
    return output


def test_update_custom_slots(
    vm: VMHandle,
    rugix: RugixCtrl,
    slots_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Install incremental file and custom payloads, retaining only the file index."""
    vm.run(["mkdir", "-p", "/run/rugix/state/app/custom-dir"], hide=True)

    for path in (
        "/run/rugix/state/test-file",
        "/run/rugix/state/app/custom-dir/test-file",
    ):
        res = vm.run(["test", "-e", path], check=False, hide=True)
        assert not res.ok, f"{path} must not exist before install"

    rugix.update_install(bundle_url(slots_bundle))

    vm.run(["test", "-e", "/run/rugix/state/test-file"], hide=True)
    vm.run(["test", "-e", "/run/rugix/state/app/custom-dir/test-file"], hide=True)

    index_dir = "/run/rugix/mounts/data/rugix/slots"
    index_name = "casync-64_sha512-256.rugix-block-index"
    vm.run(["test", "-e", f"{index_dir}/test-file/{index_name}"], hide=True)
    vm.run(["test", "!", "-e", f"{index_dir}/custom-dir/{index_name}"], hide=True)
    vm.run(["test", "!", "-e", f"{index_dir}/system-a/{index_name}"], hide=True)
