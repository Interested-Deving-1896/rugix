"""Incremental bundles deliver payloads to custom slots and execute scripts.

The ``customized-slots`` system declares a ``test-file`` slot backed by a
file and a ``custom-dir`` slot whose handler extracts a tar archive.
"""

from __future__ import annotations

import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, RugixCtrl, VMHandle

INDEX = (
    "/run/rugix/mounts/data/rugix/slots/{slot}/casync-64_sha512-256.rugix-block-index"
)


@pytest.fixture
def boot_system() -> str:
    return "customized-slots"


@pytest.fixture(scope="module")
def payloads(rugix_project: Path) -> Path:
    return rugix_project / "bundles" / "slots-bundle" / "payloads"


@pytest.fixture(scope="module")
def slots_bundle(bundler: Bundler, rugix_project: Path, payloads: Path) -> Path:
    test_tar = payloads / "test-dir.tar"
    test_tar.unlink(missing_ok=True)
    subprocess.run(
        ["tar", "-cf", str(test_tar), "-C", str(payloads), "test-file"], check=True
    )
    output = rugix_project / "build" / "slots-bundle.rugixb"
    output.unlink(missing_ok=True)
    return bundler.bundle(rugix_project / "bundles" / "slots-bundle", output)


@pytest.fixture(scope="module")
def script_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "build" / "script-bundle.rugixb"
    output.unlink(missing_ok=True)
    return bundler.bundle(rugix_project / "bundles" / "script-bundle", output)


def test_incremental_payloads(
    vm: VMHandle,
    rugix: RugixCtrl,
    payloads: Path,
    slots_bundle: Path,
    script_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Payloads reach their slots with their content, only the file slot keeps
    a block index, and an execute payload runs its script."""
    expected = (payloads / "test-file").read_text()
    vm.run(["mkdir", "-p", "/run/rugix/state/app/custom-dir"], hide=True)

    rugix.update_install(bundle_url(slots_bundle))
    for path in (
        "/run/rugix/state/test-file",
        "/run/rugix/state/app/custom-dir/test-file",
    ):
        assert vm.run(["cat", path], hide=True).stdout == expected
    vm.run(["test", "-e", INDEX.format(slot="test-file")], hide=True)
    vm.run(["test", "!", "-e", INDEX.format(slot="custom-dir")], hide=True)
    vm.run(["test", "!", "-e", INDEX.format(slot="system-a")], hide=True)

    rugix.update_install(bundle_url(script_bundle))
    vm.run(["test", "-e", "/run/rugix/state/the-update-script-was-here"], hide=True)
