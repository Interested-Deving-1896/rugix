"""End-to-end coverage for the privileged Rugix Ctrl daemon bridge."""

from __future__ import annotations

from pathlib import Path

import pytest
from rugix_testkit import BakeryBuilder, Bundler, CmdError, RugixCtrl, VMHandle

CTRL_CONFIG = """\
[signatures]
roots = ["/etc/rugix/root.crt"]
"""
REMOTE_BUNDLE = "/tmp/rugix-ctrl-daemon-update.rugixb"
REJECTED_BUNDLE = "/tmp/rugix-ctrl-daemon-rejected.rugixb"


@pytest.fixture(scope="session")
def daemon_signed_bundle(
    bakery: BakeryBuilder, bundler: Bundler, rugix_project: Path, update_keys: Path
) -> Path:
    base = bakery.bake_bundle("customized-amd64")
    output = rugix_project / "build" / "customized-amd64-daemon-signed.rugixb"
    bundler.sign(
        base,
        update_keys / "signer.crt",
        update_keys / "signer.key",
        output,
    )
    return output


@pytest.fixture
def ctrl_daemon_vm(vm: VMHandle) -> VMHandle:
    vm.run(
        [
            "sh",
            "-c",
            f"cat > /etc/rugix/ctrl.toml <<'TOML'\n{CTRL_CONFIG}TOML",
        ],
        hide=True,
    )
    vm.run(
        ["systemctl", "is-enabled", "--quiet", "rugix-ctrl-daemon.service"],
        hide=True,
    )
    vm.run(
        ["systemctl", "is-active", "--quiet", "rugix-ctrl-daemon.service"],
        hide=True,
    )
    vm.run(
        ["systemctl", "restart", "rugix-ctrl-daemon.service"],
        hide=True,
    )
    vm.run(
        [
            "sh",
            "-c",
            (
                "for attempt in $(seq 1 100); do "
                "[ -S /run/rugix/ctrl.sock ] && exit 0; sleep 0.1; "
                "done; exit 1"
            ),
        ],
        hide=True,
    )
    vm.run(
        [
            "sh",
            "-c",
            (
                "getent group rugix-daemon >/dev/null; "
                "id rugix-admin >/dev/null 2>&1 || "
                "useradd --system --gid rugix-daemon --no-create-home rugix-admin"
            ),
        ],
        hide=True,
    )
    return vm


def test_ctrl_daemon_update_installation(
    ctrl_daemon_vm: VMHandle,
    rugix: RugixCtrl,
    daemon_signed_bundle: Path,
) -> None:
    """Install a verified update as an unprivileged daemon client."""
    rugix.assert_boot(default="a", active="a")

    ctrl_daemon_vm.upload(daemon_signed_bundle, REMOTE_BUNDLE)
    ctrl_daemon_vm.run(["chmod", "644", REMOTE_BUNDLE], hide=True)
    ctrl_daemon_vm.run(
        [
            "runuser",
            "-u",
            "rugix-admin",
            "--",
            "rugix-ctrl",
            "update",
            "install",
            "--reboot",
            "no",
            REMOTE_BUNDLE,
        ],
        timeout=600,
        hide=True,
    )
    rugix.assert_boot(default="a", active="a")

    ctrl_daemon_vm.run(
        [
            "sh",
            "-c",
            (
                "nohup sh -c 'sleep 1; rugix-ctrl system reboot --spare' "
                ">/tmp/rugix-ctrl-reboot.log 2>&1 </dev/null &"
            ),
        ],
        hide=True,
    )
    ctrl_daemon_vm.wait_for_reboot()

    rugix.assert_boot(default="a", active="b")


def test_ctrl_daemon_query_and_admission_policy(
    ctrl_daemon_vm: VMHandle,
) -> None:
    """Query the recipe-configured daemon and verify its secure default policy."""
    daemon_info = ctrl_daemon_vm.run_json(
        [
            "runuser",
            "-u",
            "rugix-admin",
            "--",
            "rugix-ctrl",
            "daemon",
            "info",
            "--json",
        ]
    )
    assert daemon_info == {
        "dangerouslyInsecure": False,
        "features": {
            "factoryReset": False,
            "systemCommit": False,
            "systemReboot": False,
            "appLifecycle": False,
        },
    }

    info = ctrl_daemon_vm.run_json(
        [
            "runuser",
            "-u",
            "rugix-admin",
            "--",
            "rugix-ctrl",
            "system",
            "info",
            "--json",
        ]
    )
    assert info["boot"]["activeGroup"] == "a"
    assert info["boot"]["defaultGroup"] == "a"

    ctrl_daemon_vm.run(["touch", REJECTED_BUNDLE], hide=True)
    ctrl_daemon_vm.run(["chmod", "644", REJECTED_BUNDLE], hide=True)
    with pytest.raises(CmdError) as insecure_install:
        ctrl_daemon_vm.run(
            [
                "runuser",
                "-u",
                "rugix-admin",
                "--",
                "rugix-ctrl",
                "update",
                "install",
                "--reboot",
                "no",
                "--insecure-skip-bundle-verification",
                REJECTED_BUNDLE,
            ],
            timeout=300,
            hide=True,
        )
    assert "dangerously-insecure = true" in insecure_install.value.result.stderr

    with pytest.raises(CmdError) as system_commit:
        ctrl_daemon_vm.run(
            [
                "runuser",
                "-u",
                "rugix-admin",
                "--",
                "rugix-ctrl",
                "system",
                "commit",
            ],
            hide=True,
        )
    assert "features.system-commit = true" in system_commit.value.result.stderr
