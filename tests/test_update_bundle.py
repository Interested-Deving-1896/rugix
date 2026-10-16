"""Port of ``tests/tests/test-update-bundle.toml``.

Drives three A/B update cycles after asserting the bundle is rejected
without ``--insecure-skip-bundle-verification``.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, CmdError, RugixCtrl, VMHandle


def test_update_bundle(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    bundle = bundle_url(bakery.bake_bundle("customized-amd64"))

    # Install must fail without --insecure-skip-bundle-verification.
    with pytest.raises(CmdError):
        rugix.update_install(bundle, insecure=False, timeout=300)

    rugix.assert_boot(default="a", active="a")

    expected = [
        ("a", "b", "b"),
        ("b", "a", "a"),
        ("a", "b", "b"),
    ]
    for default_before, active, default_after in expected:
        rugix.install_and_reboot(bundle)
        rugix.assert_boot(default=default_before, active=active)
        rugix.system_commit()
        rugix.assert_boot(default=default_after, active=active)
