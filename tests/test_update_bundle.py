"""System updates boot into the updated group, roll back unless committed, and
refuse bundles that fail verification."""

from __future__ import annotations

import shutil
from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, CmdError, RugixCtrl, VMHandle

from support import BASE_PAGE, UPDATE_PAGE, web_page

FAILING_HOOK = "/etc/rugix/hooks/system-commit/pre-commit/10-fail"


@pytest.fixture(scope="module")
def update_bundle(bakery: BakeryBuilder) -> Path:
    """Bundle of a system whose web page differs from the booted one."""
    return bakery.bake_bundle("customized-amd64-delta")


@pytest.fixture(scope="module")
def corrupted_bundle(update_bundle: Path) -> Path:
    """Copy of the update bundle with one byte flipped in its payloads."""
    corrupted = update_bundle.with_name("corrupted.rugixb")
    shutil.copyfile(update_bundle, corrupted)
    with corrupted.open("r+b") as file:
        file.seek(corrupted.stat().st_size // 2)
        byte = file.read(1)
        file.seek(-1, 1)
        file.write(bytes([byte[0] ^ 0xFF]))
    return corrupted


def test_update_rolls_back_unless_committed(
    vm: VMHandle,
    rugix: RugixCtrl,
    update_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """A trial boot of an update falls back to the committed system on the next
    reboot. Once committed, the update stays booted."""
    url = bundle_url(update_bundle)
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)

    rugix.install_and_reboot(url)
    rugix.assert_boot(default="a", active="b")
    assert UPDATE_PAGE in web_page(vm)

    vm.reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)

    rugix.install_and_reboot(url)
    rugix.assert_boot(default="a", active="b")

    # A failing pre-commit hook prevents the commit.
    vm.run(["sh", "-c", f"mkdir -p {Path(FAILING_HOOK).parent}"], hide=True)
    vm.run(["sh", "-c", f"printf '#!/bin/sh\\nexit 1\\n' >{FAILING_HOOK}"], hide=True)
    vm.run(["chmod", "+x", FAILING_HOOK], hide=True)
    with pytest.raises(CmdError):
        rugix.system_commit()
    rugix.assert_boot(default="a", active="b")
    vm.run(["rm", FAILING_HOOK], hide=True)

    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
    vm.reboot()
    rugix.assert_boot(default="b", active="b")
    assert UPDATE_PAGE in web_page(vm)


def test_rejected_bundles_leave_system_unchanged(
    vm: VMHandle,
    rugix: RugixCtrl,
    update_bundle: Path,
    corrupted_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Unsigned bundles and bundles that do not match the expected hash are
    refused without selecting the spare system for the next boot."""
    with pytest.raises(CmdError) as unsigned:
        rugix.update_install(bundle_url(update_bundle), reboot="set", insecure=False)
    assert "bundle verification failed" in unsigned.value.result.stderr

    bundle_hash = update_bundle.with_suffix(".rugixb-hash").read_text().strip()
    corrupted = vm.run(
        [
            "rugix-ctrl",
            "update",
            "install",
            "--reboot",
            "set",
            "--bundle-hash",
            bundle_hash,
            bundle_url(corrupted_bundle),
        ],
        check=False,
        hide=True,
        timeout=600,
    )
    assert not corrupted.ok

    vm.reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)
