"""Binary apps run as systemd services that Rugix Ctrl manages.

The ``hello-binary`` app writes a marker file and keeps running.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, VMHandle

from support import apps, install_app, wait_for_app_state

APP_NAME = "hello-binary"
MARKER = f"/run/rugix/state/apps/{APP_NAME}/data/running"
SERVICE = f"rugix-app-{APP_NAME}.service"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-apps"


@pytest.fixture(scope="module")
def binary_app_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "apps" / "build" / "binary-hello_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    return bundler.apps_pack(
        "binary",
        ["--app", APP_NAME, "--service", "apps/binary-hello/hello-server.service"],
        rugix_project / "apps" / "binary-hello" / "hello-server",
        output,
    )


def test_binary_app_lifecycle(
    vm: VMHandle,
    binary_app_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Install, deactivate, and activate the app. Its service starts again
    after a reboot. Removing the app removes it from the list."""
    assert apps(vm) == {}

    install_app(vm, bundle_url(binary_app_bundle))
    wait_for_app_state(vm, APP_NAME, "running")
    vm.run(["systemctl", "is-active", SERVICE], hide=True)
    vm.run(["test", "-f", MARKER], hide=True)
    info = vm.run_json(["rugix-ctrl", "apps", "info", APP_NAME])
    assert info["orchestrator"] == "binary"
    assert info["lifecycle"]["state"] == "active"
    assert info["lifecycle"]["generation"] == 1
    assert [entry["active"] for entry in info["generations"]] == [True]

    vm.run(["rugix-ctrl", "apps", "deactivate", APP_NAME], hide=True)
    wait_for_app_state(vm, APP_NAME, "stopped")
    assert apps(vm)[APP_NAME].get("generation") is None
    assert not vm.run(["test", "-f", MARKER], check=False, hide=True).ok

    vm.run(["rugix-ctrl", "apps", "activate", APP_NAME, "1"], hide=True)
    wait_for_app_state(vm, APP_NAME, "running")

    vm.reboot()
    wait_for_app_state(vm, APP_NAME, "running")
    vm.run(["systemctl", "is-active", SERVICE], hide=True)
    assert apps(vm)[APP_NAME]["generation"] == 1

    vm.run(["rugix-ctrl", "apps", "remove", APP_NAME], hide=True)
    assert apps(vm) == {}
