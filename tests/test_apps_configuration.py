"""Device-specific application configuration across app generations.

Boots ``customized-amd64-apps`` and walks the ``hello-config`` app through
the configuration lifecycle: the bundled default applies until the device sets
its own document, an unchanged document does not restart the workload, a
generation whose schema no longer accepts the stored document is refused
without disturbing the running app, ``apps activate --config`` takes that
generation over with a matching document, and rollback restores the generation
together with the revision it ran.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from rugix_testkit import Bundler, VMHandle

from support import app_info, apps, install_app, wait_for_app_state

APP_NAME = "hello-config"

DEVICE_CONFIG = {"endpoint": "https://device.example.com"}
DEVICE_CONFIG_V2 = {"endpoint": "https://device.example.com", "token": "device-token"}

DEVICE_CONFIG_PATH = "/tmp/hello-config.json"
DEVICE_CONFIG_V2_PATH = "/tmp/hello-config-v2.json"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-apps"


@pytest.fixture(scope="session")
def app_dir(rugix_project: Path) -> Path:
    return rugix_project / "apps" / "generic-config"


def _pack(
    bundler: Bundler,
    app_dir: Path,
    rugix_project: Path,
    *,
    label: str,
    schema: str,
    default: str,
) -> Path:
    output = rugix_project / "apps" / "build" / f"generic-config_{label}_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.unlink(missing_ok=True)
    bundler.apps_pack(
        "generic",
        [
            "--app",
            APP_NAME,
            "--config-schema",
            str((app_dir / schema).relative_to(rugix_project)),
            "--config-default",
            str((app_dir / default).relative_to(rugix_project)),
        ],
        app_dir / "orchestrator",
        output,
    )
    return output


@pytest.fixture(scope="session")
def app_bundle_v1(bundler: Bundler, app_dir: Path, rugix_project: Path) -> Path:
    """Bundle whose schema requires only ``endpoint``."""
    return _pack(
        bundler,
        app_dir,
        rugix_project,
        label="v1",
        schema="config.schema.json",
        default="config.default.json",
    )


@pytest.fixture(scope="session")
def app_bundle_v2(bundler: Bundler, app_dir: Path, rugix_project: Path) -> Path:
    """Bundle whose schema additionally requires ``token``."""
    return _pack(
        bundler,
        app_dir,
        rugix_project,
        label="v2",
        schema="config.schema.v2.json",
        default="config.default.v2.json",
    )


def test_apps_configuration(
    vm: VMHandle,
    app_bundle_v1: Path,
    app_bundle_v2: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    """Configuration documents follow the app through installation,
    activation, and rollback of its generations."""
    _install(vm, bundle_url(app_bundle_v1))

    # Without a device-specific document, the bundled default applies.
    assert _applied_config(vm) == {"endpoint": "https://default.example.com"}
    assert _config_get(vm) == {"endpoint": "https://default.example.com"}
    assert _generation(vm) == 1
    assert _orchestrator(vm) == "generic"
    assert _configuration_revision(vm) is None
    assert _desired_configuration_revision(vm) is None

    _write_json(vm, DEVICE_CONFIG_PATH, DEVICE_CONFIG)
    assert _config_set(vm, DEVICE_CONFIG_PATH) == 1
    assert _applied_config(vm) == DEVICE_CONFIG
    assert _configuration_revision(vm) == 1
    assert _desired_configuration_revision(vm) == 1

    # Re-applying the same document reuses its revision and leaves the running
    # workload in place.
    pid = _workload_pid(vm)
    assert _config_set(vm, DEVICE_CONFIG_PATH) == 1
    assert _workload_pid(vm) == pid

    # Generation 2 requires ``token``, which the stored document does not have, so
    # the installation is refused and the app keeps running generation 1.
    result = vm.run(
        [
            "rugix-ctrl",
            "apps",
            "install",
            "--insecure-skip-bundle-verification",
            bundle_url(app_bundle_v2),
        ],
        check=False,
        timeout=300,
        hide=True,
    )
    assert not result.ok
    assert "does not match its schema" in result.stderr
    assert _generation(vm) == 1
    assert _configuration_revision(vm) == 1
    assert _applied_config(vm) == DEVICE_CONFIG
    assert _status(vm) == "running"

    # The refused generation was installed completely, so it can be taken over
    # with a matching document without downloading the bundle again.
    generations = {entry["number"]: entry for entry in _info(vm)["generations"]}
    assert generations[2]["complete"]
    assert not generations[2]["active"]

    _write_json(vm, DEVICE_CONFIG_V2_PATH, DEVICE_CONFIG_V2)
    vm.run(
        [
            "rugix-ctrl",
            "apps",
            "activate",
            APP_NAME,
            "2",
            "--config",
            DEVICE_CONFIG_V2_PATH,
        ],
        timeout=120,
        hide=True,
    )
    wait_for_app_state(vm, APP_NAME, "running")
    assert _generation(vm) == 2
    assert _configuration_revision(vm) == 2
    assert _applied_config(vm) == DEVICE_CONFIG_V2
    assert _status(vm) == "running"

    # Rollback restores generation 1 together with the revision it ran, rather
    # than the newest revision, which generation 1's schema would reject.
    vm.run(["rugix-ctrl", "apps", "rollback", APP_NAME], timeout=120, hide=True)
    wait_for_app_state(vm, APP_NAME, "running")
    assert _generation(vm) == 1
    assert _configuration_revision(vm) == 1
    assert _applied_config(vm) == DEVICE_CONFIG
    assert _status(vm) == "running"


def _install(vm: VMHandle, url: str) -> None:
    install_app(vm, url)
    wait_for_app_state(vm, APP_NAME, "running")


def _write_json(vm: VMHandle, path: str, document: dict[str, Any]) -> None:
    script = f"cat > {path} <<'EOF_CONFIG'\n{json.dumps(document)}\nEOF_CONFIG\n"
    vm.run(["sh", "-c", script], hide=True)


def _config_set(vm: VMHandle, path: str) -> Any:
    output = vm.run_json(["rugix-ctrl", "apps", "config", "set", APP_NAME, path])
    return output["revision"]


def _config_get(vm: VMHandle) -> Any:
    return vm.run_json(["rugix-ctrl", "apps", "config", "get", APP_NAME])


def _info(vm: VMHandle) -> Any:
    return app_info(vm, APP_NAME)


def _generation(vm: VMHandle) -> Any:
    return apps(vm)[APP_NAME].get("generation")


def _status(vm: VMHandle) -> Any:
    return apps(vm)[APP_NAME]["status"]["state"]


def _configuration_revision(vm: VMHandle) -> Any:
    return _info(vm)["lifecycle"].get("configurationRevision")


def _desired_configuration_revision(vm: VMHandle) -> Any:
    return _info(vm).get("desiredConfigurationRevision")


def _orchestrator(vm: VMHandle) -> Any:
    return apps(vm)[APP_NAME].get("orchestrator")


def _applied_config(vm: VMHandle) -> Any:
    path = f"/run/rugix/state/apps/{APP_NAME}/data/applied-config.json"
    return json.loads(vm.run(["cat", path], hide=True).stdout)


def _workload_pid(vm: VMHandle) -> str:
    path = f"/run/rugix/state/apps/{APP_NAME}/data/pid"
    return vm.run(["cat", path], hide=True).stdout.strip()
