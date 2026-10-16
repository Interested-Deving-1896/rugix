"""Signed bundles install only when a trusted root certificate verifies them."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import CmdError, RugixCtrl, VMHandle

from support import trust_test_root


def test_signed_bundle_requires_trusted_root(
    vm: VMHandle,
    rugix: RugixCtrl,
    signed_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """A signed bundle is refused with an unrelated root certificate and
    installs with the configured root."""
    trust_test_root(vm)
    url = bundle_url(signed_bundle)

    with pytest.raises(CmdError) as untrusted:
        rugix.update_install(url, insecure=False, root_cert="/etc/rugix/other-root.crt")
    assert "bundle verification failed" in untrusted.value.result.stderr

    rugix.install_and_reboot(url, insecure=False)
    rugix.assert_boot(default="a", active="b")
