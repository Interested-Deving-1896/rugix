"""Port of ``tests/tests/test-apps-generic.toml``.

Boots ``customized-amd64-docker``, packs the ``hello-generic`` orchestrator
app, and walks through install / activate / deactivate / start / stop /
remove. Adds a ``stop`` + ``start`` sequence on top of what
``test_apps_binary`` covers — the workload can be paused without
deactivating the app.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, VMHandle

APP_NAME = "hello-generic"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-docker"


@pytest.fixture
def boot_timeout() -> float:
    return 600.0


@pytest.fixture(scope="session")
def generic_app_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "apps" / "build" / "generic-hello_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        output.unlink()
    bundler.apps_pack(
        "generic",
        ["--app", APP_NAME],
        rugix_project / "apps" / "generic-hello" / "orchestrator",
        output,
    )
    return output


def test_apps_generic(
    vm: VMHandle,
    generic_app_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    _assert_no_apps(vm)

    vm.run(
        [
            "rugix-ctrl",
            "apps",
            "install",
            "--insecure-skip-bundle-verification",
            bundle_url(generic_app_bundle),
        ],
        timeout=300,
        hide=True,
    )
    _wait(vm, 3)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"
    assert apps[APP_NAME]["generation"] == 1

    marker = vm.run(
        ["cat", f"/run/rugix/state/apps/{APP_NAME}/data/running"], hide=True
    ).stdout.strip()
    assert marker == APP_NAME

    vm.run(["rugix-ctrl", "apps", "deactivate", APP_NAME], hide=True)
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "stopped"
    # ``jq '.x == null'`` returns true for both null and missing keys; mirror
    # that with ``.get`` so a missing field passes alongside an explicit null.
    assert apps[APP_NAME].get("generation") is None

    vm.run(["rugix-ctrl", "apps", "activate", APP_NAME, "1"], hide=True)
    _wait(vm, 3)
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"

    # Stop the workload, but keep it active.
    vm.run(["rugix-ctrl", "apps", "stop", APP_NAME], hide=True)
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "stopped"
    assert apps[APP_NAME]["generation"] == 1

    res = vm.run(
        ["test", "-f", f"/run/rugix/state/apps/{APP_NAME}/data/running"],
        check=False,
        hide=True,
    )
    assert not res.ok

    vm.run(["rugix-ctrl", "apps", "start", APP_NAME], hide=True)
    _wait(vm, 3)
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"
    assert apps[APP_NAME]["generation"] == 1

    vm.run(["rugix-ctrl", "apps", "remove", APP_NAME], hide=True)
    _assert_no_apps(vm)


def _assert_no_apps(vm: VMHandle) -> None:
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert isinstance(apps, dict) and len(apps) == 0


def _wait(vm: VMHandle, seconds: int) -> None:
    vm.run(["sleep", str(seconds)], hide=True)
