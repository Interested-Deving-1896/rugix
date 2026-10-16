"""The privileged Rugix Ctrl daemon serves unprivileged clients within its policy.

Members of the ``rugix-daemon`` group are daemon clients. The base layer
installs the daemon with its default policy, which permits update
installation but disables insecure operations and all optional features.
"""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest
from rugix_testkit import BakeryBuilder, CmdError, CmdResult, RugixCtrl, VMHandle

CLIENT = "rugix-client"
CTRL_CONFIG = """\
[signatures]
roots = ["/etc/rugix/root.crt"]
"""
REMOTE_BUNDLE = "/tmp/signed.rugixb"


@pytest.fixture
def daemon_vm(vm: VMHandle) -> VMHandle:
    """A VM whose daemon trusts the test root and that has a client user."""
    vm.run(
        ["sh", "-c", f"cat >/etc/rugix/ctrl.toml <<'TOML'\n{CTRL_CONFIG}TOML"],
        hide=True,
    )
    vm.run(["systemctl", "restart", "rugix-ctrl-daemon.service"], hide=True)
    vm.run(
        ["sh", "-c", "until [ -S /run/rugix/ctrl.sock ]; do sleep 0.1; done"],
        timeout=30,
        hide=True,
    )
    vm.run(["useradd", "--system", "--gid", "rugix-daemon", CLIENT], hide=True)
    return vm


def test_client_installs_verified_update(
    daemon_vm: VMHandle,
    rugix: RugixCtrl,
    signed_bundle: Path,
) -> None:
    """A client installs a signed bundle streamed from a file it can read."""
    daemon_vm.upload(signed_bundle, REMOTE_BUNDLE)
    daemon_vm.run(["chmod", "644", REMOTE_BUNDLE], hide=True)
    _as_client(daemon_vm, ["update", "install", "--reboot", "no", REMOTE_BUNDLE])
    rugix.assert_boot(default="a", active="a")

    daemon_vm.run_disconnecting(["rugix-ctrl", "system", "reboot", "--spare"])
    daemon_vm.wait_for_reboot()
    rugix.assert_boot(default="a", active="b")


def test_daemon_enforces_policy(
    daemon_vm: VMHandle,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """Clients can query the system but not bypass verification, use disabled
    features, or connect without group membership."""
    daemon_info = _as_client_json(daemon_vm, ["daemon", "info", "--json"])
    assert daemon_info == {
        "dangerouslyInsecure": False,
        "features": {
            "factoryReset": False,
            "systemCommit": False,
            "systemReboot": False,
            "appLifecycle": False,
        },
    }
    boot = _as_client_json(daemon_vm, ["system", "info", "--json"])["boot"]
    assert (boot["defaultGroup"], boot["activeGroup"]) == ("a", "a")

    unsigned = bundle_url(bakery.bake_bundle("customized-amd64"))
    with pytest.raises(CmdError) as unverified:
        _as_client(daemon_vm, ["update", "install", "--reboot", "no", unsigned])
    assert "bundle verification failed" in unverified.value.result.stderr

    with pytest.raises(CmdError) as insecure:
        _as_client(
            daemon_vm,
            ["update", "install", "--insecure-skip-bundle-verification", unsigned],
        )
    assert "dangerously-insecure = true" in insecure.value.result.stderr

    with pytest.raises(CmdError) as commit:
        _as_client(daemon_vm, ["system", "commit"])
    assert "features.system-commit = true" in commit.value.result.stderr

    outsider = daemon_vm.run(
        ["runuser", "-u", "nobody", "--", "rugix-ctrl", "system", "info"],
        check=False,
        hide=True,
    )
    assert not outsider.ok


def _as_client(vm: VMHandle, args: list[str]) -> CmdResult:
    return vm.run(
        ["runuser", "-u", CLIENT, "--", "rugix-ctrl", *args], timeout=600, hide=True
    )


def _as_client_json(vm: VMHandle, args: list[str]) -> Any:
    return vm.run_json(["runuser", "-u", CLIENT, "--", "rugix-ctrl", *args])
