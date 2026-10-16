"""Generic apps delegate their lifecycle to an orchestrator script.

The ``hello-generic`` orchestrator starts a background process that writes a
marker file.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, VMHandle

from support import apps, install_app, wait_for_app_state

APP_NAME = "hello-generic"
MARKER = f"/run/rugix/state/apps/{APP_NAME}/data/running"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-apps"


@pytest.fixture(scope="module")
def generic_app_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "apps" / "build" / "generic-hello_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    return bundler.apps_pack(
        "generic",
        ["--app", APP_NAME],
        rugix_project / "apps" / "generic-hello" / "orchestrator",
        output,
    )


def test_generic_app_lifecycle(
    vm: VMHandle,
    generic_app_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Deactivating the app clears its generation. Stopping it keeps the
    generation active while the workload is not running."""
    install_app(vm, bundle_url(generic_app_bundle))
    wait_for_app_state(vm, APP_NAME, "running")
    assert apps(vm)[APP_NAME]["generation"] == 1
    vm.run(["test", "-f", MARKER], hide=True)

    vm.run(["rugix-ctrl", "apps", "deactivate", APP_NAME], hide=True)
    wait_for_app_state(vm, APP_NAME, "stopped")
    assert apps(vm)[APP_NAME].get("generation") is None

    vm.run(["rugix-ctrl", "apps", "activate", APP_NAME, "1"], hide=True)
    wait_for_app_state(vm, APP_NAME, "running")

    vm.run(["rugix-ctrl", "apps", "stop", APP_NAME], hide=True)
    wait_for_app_state(vm, APP_NAME, "stopped")
    assert apps(vm)[APP_NAME]["generation"] == 1
    assert not vm.run(["test", "-f", MARKER], check=False, hide=True).ok

    vm.run(["rugix-ctrl", "apps", "start", APP_NAME], hide=True)
    wait_for_app_state(vm, APP_NAME, "running")

    vm.run(["rugix-ctrl", "apps", "remove", APP_NAME], hide=True)
    assert apps(vm) == {}
