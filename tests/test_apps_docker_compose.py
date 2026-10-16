"""Port of ``tests/tests/test-apps-docker-compose.toml``.

Packs ``apps/docker-compose-website`` via ``bundler apps pack
docker-compose --pull``, installs into ``customized-amd64-docker``, and
walks through the install / curl / activate / reboot / remove flow.
Deactivation and removal must remove the app's Rugix-owned image tags. The
bundled image is kept in the content-addressed store, so reactivation loads
it from there.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

import pytest
from rugix_testkit import Bundler, VMHandle

APP_NAME = "website"
APP_IMAGE_PREFIX = f"localhost/rugix-apps/{APP_NAME}/"
URL = "http://localhost:8080/"
EXPECTED_BODY = "Hello from Rugix Apps"


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-docker"


@pytest.fixture
def boot_timeout() -> float:
    return 600.0


@pytest.fixture(scope="session")
def website_bundle(bundler: Bundler, rugix_project: Path) -> Path:
    output = rugix_project / "apps" / "build" / "docker-compose-website_amd64.rugixb"
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        output.unlink()
    bundler.apps_pack(
        "docker-compose",
        [
            "--pull",
            "--platform",
            "linux/amd64",
            "--app",
            APP_NAME,
            "--include",
            "apps/docker-compose-website/www",
        ],
        rugix_project / "apps" / "docker-compose-website" / "docker-compose.yml",
        output,
    )
    return output


def test_apps_docker_compose(
    vm: VMHandle,
    website_bundle: Path,
    bundle_url: Callable[[Path], str],
) -> None:
    vm.run(["docker", "--version"], hide=True)
    vm.run(["docker", "compose", "version"], hide=True)
    _assert_no_apps(vm)

    vm.run(
        [
            "rugix-ctrl",
            "apps",
            "install",
            "--insecure-skip-bundle-verification",
            bundle_url(website_bundle),
        ],
        timeout=600,
        hide=True,
    )
    _wait(vm, 5)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"
    assert apps[APP_NAME]["generation"] == 1
    assert _app_images(vm) != []

    files = json.loads(
        vm.run(["rugix-ctrl", "apps", "files", APP_NAME], hide=True).stdout
    )
    images = [file for file in files if file["path"].startswith("images/")]
    assert images and all("object" in image for image in images)
    store = vm.run_json(["rugix-ctrl", "store", "info"])
    stored, referenced = store["storedSize"], store["referencedSize"]
    assert store["refs"] == len(images)
    assert (
        isinstance(stored, int) and isinstance(referenced, int) and stored < referenced
    )
    assert vm.run_json(["rugix-ctrl", "store", "verify"])["problems"] == []

    _wait(vm, 2)
    body = vm.run(["curl", "-sf", URL], hide=True).stdout
    assert EXPECTED_BODY in body

    vm.run(["rugix-ctrl", "apps", "deactivate", APP_NAME], hide=True)
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "stopped"
    assert _app_images(vm) == []

    vm.run(["rugix-ctrl", "apps", "activate", APP_NAME, "1"], hide=True)
    _wait(vm, 3)
    body = vm.run(["curl", "-sf", URL], hide=True).stdout
    assert EXPECTED_BODY in body

    vm.reboot()

    # Wait for Docker to come back online.
    vm.run(
        [
            "sh",
            "-c",
            "for i in $(seq 1 60); do docker info >/dev/null 2>&1 && exit 0; sleep 1; done; exit 1",
        ],
        timeout=120,
        hide=True,
    )
    _wait(vm, 10)

    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert apps[APP_NAME]["status"]["state"] == "running"
    body = vm.run(["curl", "-sf", URL], hide=True).stdout
    assert EXPECTED_BODY in body

    # A second generation with the same images shares all stored content.
    vm.run(
        [
            "rugix-ctrl",
            "apps",
            "install",
            "--insecure-skip-bundle-verification",
            bundle_url(website_bundle),
        ],
        timeout=600,
        hide=True,
    )
    store_after_update = vm.run_json(["rugix-ctrl", "store", "info"])
    assert store_after_update["refs"] == 2 * len(images)
    assert store_after_update["storedSize"] == stored

    vm.run(["rugix-ctrl", "apps", "remove", APP_NAME], hide=True)
    _assert_no_apps(vm)
    assert _app_images(vm) == []
    store = vm.run_json(["rugix-ctrl", "store", "info"])
    assert (store["refs"], store["objects"], store["chunks"]) == (0, 0, 0)


def _assert_no_apps(vm: VMHandle) -> None:
    apps = vm.run_json(["rugix-ctrl", "apps", "list"])
    assert isinstance(apps, dict) and len(apps) == 0


def _app_images(vm: VMHandle) -> list[str]:
    output = vm.run(
        ["docker", "image", "ls", "--format", "{{.Repository}}:{{.Tag}}"], hide=True
    ).stdout
    return [image for image in output.split() if image.startswith(APP_IMAGE_PREFIX)]


def _wait(vm: VMHandle, seconds: int) -> None:
    vm.run(["sleep", str(seconds)], hide=True)
