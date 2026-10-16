"""Port of ``tests/tests/test-apps-binary.toml``.

Boots ``customized-amd64-docker``, packs the ``hello-binary`` app from
``apps/binary-hello/`` via ``bundler apps pack binary`` (one-shot binary
served by a systemd unit), and walks through install → activate/deactivate
→ reboot → remove.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, VMHandle

APP_NAME = "hello-binary"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-docker"


@pytest.fixture
def boot_timeout() -> float:
    # Docker host boots slower than the regular customized-amd64.
    return 600.0


@pytest.fixture(scope="session")
def binary_app_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "apps" / "build" / "binary-hello_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        output.unlink()
    bundler.apps_pack(
        "binary",
        [
            "--app",
            APP_NAME,
            "--service",
            "apps/binary-hello/hello-server.service",
        ],
        rugix_project / "apps" / "binary-hello" / "hello-server",
        output,
    )
    return output


def test_apps_binary(
    vm: VMHandle,
    binary_app_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    _assert_no_apps(vm)

    vm.run(
        [
            "rugix-ctrl",
            "apps",
            "install",
            "--insecure-skip-bundle-verification",
            bundle_url(binary_app_bundle),
        ],
        timeout=300,
        hide=True,
    )
    _wait(vm, 3)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    app = apps[APP_NAME]
    assert isinstance(app, dict)
    assert app["status"]["state"] == "running"
    assert app["generation"] == 1

    marker = vm.run(
        ["cat", f"/run/rugix/state/apps/{APP_NAME}/data/running"], hide=True
    ).stdout.strip()
    assert marker == APP_NAME

    vm.run(["systemctl", "is-active", f"rugix-app-{APP_NAME}.service"], hide=True)

    info = vm.run_json(["rugix-ctrl", "apps", "info", APP_NAME])
    assert info["name"] == APP_NAME
    assert info["orchestrator"] == "binary"
    assert info["lifecycle"]["state"] == "active"
    assert info["lifecycle"]["generation"] == 1
    assert isinstance(info["generations"], list) and len(info["generations"]) == 1
    assert info["generations"][0]["active"] is True

    vm.run(["rugix-ctrl", "apps", "deactivate", APP_NAME], hide=True)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "stopped"
    # ``jq '.x == null'`` returns true for both null and missing keys; mirror
    # that with ``.get`` so a missing field passes alongside an explicit null.
    assert apps[APP_NAME].get("generation") is None

    res = vm.run(
        ["test", "-f", f"/run/rugix/state/apps/{APP_NAME}/data/running"],
        check=False,
        hide=True,
    )
    assert not res.ok

    vm.run(["rugix-ctrl", "apps", "activate", APP_NAME, "1"], hide=True)
    _wait(vm, 3)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"

    vm.reboot()
    _wait(vm, 10)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"
    assert apps[APP_NAME]["generation"] == 1
    vm.run(["systemctl", "is-active", f"rugix-app-{APP_NAME}.service"], hide=True)
    marker = vm.run(
        ["cat", f"/run/rugix/state/apps/{APP_NAME}/data/running"], hide=True
    ).stdout.strip()
    assert marker == APP_NAME

    vm.run(["rugix-ctrl", "apps", "remove", APP_NAME], hide=True)
    _assert_no_apps(vm)


def _assert_no_apps(vm: VMHandle) -> None:
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert isinstance(apps, dict) and len(apps) == 0


def _wait(vm: VMHandle, seconds: int) -> None:
    vm.run(["sleep", str(seconds)], hide=True)
