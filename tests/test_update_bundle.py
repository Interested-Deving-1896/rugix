"""System updates boot into the updated group, roll back unless committed, and
refuse bundles that fail verification."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, CmdError, RugixCtrl, VMHandle

from support import BASE_PAGE, UPDATE_PAGE, web_page

HOOKS_DIR = "/etc/rugix/hooks/system-commit/pre-commit"
WRONG_BUNDLE_HASH = "sha512-256:" + "0" * 64


@pytest.fixture(scope="module")
def update_bundle(bakery: BakeryBuilder) -> Path:
    """Bundle of a system whose web page differs from the booted one."""
    return bakery.bake_bundle("customized-amd64-delta")


def test_update_rolls_back_unless_committed(
    vm: VMHandle,
    rugix: RugixCtrl,
    update_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """A trial boot of an update falls back to the committed system on the next
    reboot. A failing pre-commit hook prevents the commit. Once committed, the
    update stays booted."""
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

    vm.run(
        [
            "sh",
            "-c",
            f"mkdir -p {HOOKS_DIR} && printf '#!/bin/sh\\nexit 1\\n' >{HOOKS_DIR}/10-fail",
        ],
        hide=True,
    )
    vm.run(["chmod", "+x", f"{HOOKS_DIR}/10-fail"], hide=True)
    with pytest.raises(CmdError) as commit:
        rugix.system_commit()
    assert 'hook "system-commit/pre-commit/fail" failed' in commit.value.result.stderr
    rugix.assert_boot(default="a", active="b")
    vm.run(["rm", f"{HOOKS_DIR}/10-fail"], hide=True)

    rugix.system_commit()
    rugix.assert_boot(default="b", active="b")
    vm.reboot()
    rugix.assert_boot(default="b", active="b")
    assert UPDATE_PAGE in web_page(vm)


def test_rejected_bundles_leave_system_unchanged(
    vm: VMHandle,
    rugix: RugixCtrl,
    update_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Unsigned bundles and bundles that do not match the expected hash are
    refused without selecting the spare system for the next boot."""
    url = bundle_url(update_bundle)
    with pytest.raises(CmdError) as unsigned:
        rugix.update_install(url, reboot="set", insecure=False)
    assert "bundle verification failed" in unsigned.value.result.stderr

    with pytest.raises(CmdError) as mismatch:
        vm.run(
            ["rugix-ctrl", "update", "install", "--reboot", "set"]
            + ["--bundle-hash", WRONG_BUNDLE_HASH, url],
            hide=True,
            timeout=600,
        )
    assert "invalid header hash" in mismatch.value.result.stderr

    vm.reboot()
    rugix.assert_boot(default="a", active="a")
    assert BASE_PAGE in web_page(vm)
