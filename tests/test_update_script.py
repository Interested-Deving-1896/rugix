"""Port of ``tests/tests/test-update-script.toml``.

Builds ``bundles/script-bundle`` via ``bundler bundle`` and verifies that
installing it leaves the documented marker file under ``/run/rugix/state``.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, RugixCtrl, VMHandle

MARKER = "/run/rugix/state/the-update-script-was-here"


@pytest.fixture(scope="session")
def script_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "build" / "script-bundle.rugixb"
    bundler.bundle(rugix_project / "bundles" / "script-bundle", output)
    return output


def test_update_script(
    vm: VMHandle,
    rugix: RugixCtrl,
    script_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    res = vm.run(["test", "-e", MARKER], check=False, hide=True)
    assert not res.ok, f"{MARKER} must not exist before install"

    rugix.update_install(bundle_url(script_bundle))

    vm.run(["test", "-e", MARKER], hide=True)
