"""Compatibility boot flows drive firmware environment variables correctly.

The VM boots with Rugix's EFI boot flow. The tests reconfigure Rugix Ctrl to
use a compatibility boot flow and replace ``fw_printenv`` and ``fw_setenv``
with scripts that operate on a plain file, so that they can check the
variables Rugix Ctrl sets without the matching bootloader.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

from rugix_testkit import BakeryBuilder, CmdResult, VMHandle

ENVIRONMENT = "/tmp/rugix-fwenv"
BIN = "/tmp/rugix-test-bin"


def test_rauc_grub_update_state_transitions(
    vm: VMHandle,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """An update puts the spare group first in ``BOOT_ORDER`` and a commit
    restores the running group."""
    _configure_firmware_environment(
        vm,
        boot_flow="rauc-grub",
        variables={
            "BOOT_ORDER": "A B",
            "A_OK": "1",
            "A_TRY": "0",
            "B_OK": "1",
            "B_TRY": "1",
        },
    )
    boot = _boot_info(vm)
    assert (boot["bootFlow"], boot["activeGroup"], boot["defaultGroup"]) == (
        "rauc-grub",
        "a",
        "a",
    )

    _install(vm, bundle_url(bakery.bake_bundle("customized-amd64")))
    environment = _environment(vm)
    assert environment["BOOT_ORDER"] == "B A"
    assert (environment["B_OK"], environment["B_TRY"]) == ("1", "0")
    assert _boot_info(vm)["defaultGroup"] == "b"

    # The VM never rebooted, so the commit makes the running group A the
    # default again.
    _ctrl(vm, ["system", "commit"])
    assert _environment(vm)["BOOT_ORDER"] == "A B"
    assert _boot_info(vm)["defaultGroup"] == "a"


def test_mender_uboot_update_state_transitions(
    vm: VMHandle,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """An update starts a Mender trial of the spare partition, which a commit
    before the reboot leaves in place."""
    _configure_firmware_environment(
        vm,
        boot_flow="mender-uboot",
        variables={
            "bootcount": "0",
            "upgrade_available": "0",
            "mender_boot_part": "2",
            "mender_boot_part_hex": "0x2",
        },
    )
    boot = _boot_info(vm)
    assert (boot["bootFlow"], boot["activeGroup"], boot["defaultGroup"]) == (
        "mender-uboot",
        "a",
        "a",
    )

    _install(vm, bundle_url(bakery.bake_bundle("customized-amd64")))
    trial = {
        "upgrade_available": "1",
        "mender_boot_part": "3",
        "mender_boot_part_hex": "0x3",
    }
    assert _environment(vm).items() >= trial.items()
    # During a Mender trial, the committed pre-update group remains the default.
    assert _boot_info(vm)["defaultGroup"] == "a"

    # Without a U-Boot reboot the VM still runs group A, the committed group,
    # so the commit is a no-op and preserves the trial variables.
    _ctrl(vm, ["system", "commit"])
    assert _environment(vm).items() >= trial.items()


def _configure_firmware_environment(
    vm: VMHandle, *, boot_flow: str, variables: dict[str, str]
) -> None:
    config = f'[boot-flow]\ntype = "{boot_flow}"\n'
    if boot_flow.startswith("rauc-"):
        config += 'group-names = ["A", "B"]\n'
    environment = "".join(f"{key}={value}\n" for key, value in variables.items())
    setup = f"""
set -eu
mkdir -p {BIN}
cat > /etc/rugix/system.toml <<'EOF_CONFIG'
{config}EOF_CONFIG
cat > {ENVIRONMENT} <<'EOF_ENV'
{environment}EOF_ENV
cat > {BIN}/fw_printenv <<'EOF_PRINT'
#!/bin/sh
cat {ENVIRONMENT}
EOF_PRINT
cat > {BIN}/fw_setenv <<'EOF_SET'
#!/bin/sh
set -eu
updates=$(mktemp)
output=$(mktemp)
trap 'rm -f "$updates" "$output"' EXIT
cat > "$updates"
awk -F= '
  NR == FNR {{
    key=$1
    value=substr($0, length(key) + 2)
    updates[key]=value
    next
  }}
  {{
    key=$1
    if (key in updates) {{ print key "=" updates[key] }} else {{ print $0 }}
    seen[key]=1
  }}
  END {{
    for (key in updates) if (!(key in seen)) print key "=" updates[key]
  }}
' "$updates" {ENVIRONMENT} > "$output"
mv "$output" {ENVIRONMENT}
EOF_SET
cat > {BIN}/rugix-ctrl <<'EOF_CTRL'
#!/bin/sh
PATH={BIN}:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
exec /usr/bin/rugix-ctrl "$@"
EOF_CTRL
chmod +x {BIN}/fw_printenv {BIN}/fw_setenv {BIN}/rugix-ctrl
"""
    vm.run(["sh", "-c", setup], hide=True)


def _install(vm: VMHandle, url: str) -> None:
    _ctrl(
        vm,
        [
            "update",
            "install",
            "--reboot",
            "set",
            "--insecure-skip-bundle-verification",
            url,
        ],
        timeout=600,
    )


def _ctrl(vm: VMHandle, args: list[str], **kwargs: Any) -> CmdResult:
    return vm.run([f"{BIN}/rugix-ctrl", *args], hide=True, **kwargs)


def _boot_info(vm: VMHandle) -> dict[str, Any]:
    info = json.loads(_ctrl(vm, ["system", "info", "--json"]).stdout)
    return info["boot"]


def _environment(vm: VMHandle) -> dict[str, str]:
    lines = vm.run(["cat", ENVIRONMENT], hide=True).stdout.splitlines()
    return dict(line.split("=", 1) for line in lines if line)
