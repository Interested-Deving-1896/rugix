"""Compatibility boot-flow integration through stateful firmware environment adapters."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

from rugix_testkit import BakeryBuilder, CmdResult, VMHandle


def _configure_firmware_environment(
    vm: VMHandle, *, boot_flow: str, variables: dict[str, str]
) -> None:
    config = f'[boot-flow]\ntype = "{boot_flow}"\n'
    if boot_flow.startswith("rauc-"):
        config += 'group-names = ["A", "B"]\n'
    environment = "".join(f"{key}={value}\n" for key, value in variables.items())
    setup = f"""
set -eu
mkdir -p /tmp/rugix-test-bin
cat > /etc/rugix/system.toml <<'EOF_CONFIG'
{config}EOF_CONFIG
cat > /tmp/rugix-fwenv <<'EOF_ENV'
{environment}EOF_ENV
cat > /tmp/rugix-test-bin/fw_printenv <<'EOF_PRINT'
#!/bin/sh
cat /tmp/rugix-fwenv
EOF_PRINT
cat > /tmp/rugix-test-bin/fw_setenv <<'EOF_SET'
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
' "$updates" /tmp/rugix-fwenv > "$output"
mv "$output" /tmp/rugix-fwenv
EOF_SET
cat > /tmp/rugix-test-bin/rugix-ctrl <<'EOF_CTRL'
#!/bin/sh
PATH=/tmp/rugix-test-bin:/usr/sbin:/usr/bin:/sbin:/bin
export PATH
exec /usr/bin/rugix-ctrl "$@"
EOF_CTRL
chmod +x /tmp/rugix-test-bin/fw_printenv /tmp/rugix-test-bin/fw_setenv \
  /tmp/rugix-test-bin/rugix-ctrl
"""
    vm.run(["sh", "-c", setup], hide=True)


def _ctrl(vm: VMHandle, args: list[str], **kwargs: Any) -> CmdResult:
    return vm.run(["/tmp/rugix-test-bin/rugix-ctrl", *args], hide=True, **kwargs)


def _system_info(vm: VMHandle) -> dict[str, object]:
    result = _ctrl(vm, ["system", "info", "--json"])
    import json

    value = json.loads(result.stdout)
    assert isinstance(value, dict)
    return value


def _boot_info(vm: VMHandle) -> dict[str, object]:
    value = _system_info(vm)["boot"]
    assert isinstance(value, dict)
    return value


def test_rauc_grub_update_state_transitions(
    vm: VMHandle,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
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
    assert boot["bootFlow"] == "rauc-grub"
    assert boot["activeGroup"] == "a"
    assert boot["defaultGroup"] == "a"

    bundle = bundle_url(bakery.bake_bundle("customized-amd64"))
    result = _ctrl(
        vm,
        [
            "update",
            "install",
            "--reboot",
            "set",
            "--insecure-skip-bundle-verification",
            bundle,
        ],
        timeout=600,
        check=False,
    )
    assert result.ok, f"{result.stdout}\n{result.stderr}"
    environment = vm.run(["cat", "/tmp/rugix-fwenv"], hide=True).stdout
    assert "BOOT_ORDER=B A" in environment
    assert "B_OK=1" in environment
    assert "B_TRY=0" in environment
    assert _boot_info(vm)["defaultGroup"] == "b"

    _ctrl(vm, ["system", "commit"])
    environment = vm.run(["cat", "/tmp/rugix-fwenv"], hide=True).stdout
    assert "BOOT_ORDER=A B" in environment
    assert _boot_info(vm)["defaultGroup"] == "a"


def test_mender_uboot_update_state_transitions(
    vm: VMHandle,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
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
    assert boot["bootFlow"] == "mender-uboot"
    assert boot["activeGroup"] == "a"
    assert boot["defaultGroup"] == "a"

    bundle = bundle_url(bakery.bake_bundle("customized-amd64"))
    result = _ctrl(
        vm,
        [
            "update",
            "install",
            "--reboot",
            "set",
            "--insecure-skip-bundle-verification",
            bundle,
        ],
        timeout=600,
        check=False,
    )
    assert result.ok, f"{result.stdout}\n{result.stderr}"
    environment = vm.run(["cat", "/tmp/rugix-fwenv"], hide=True).stdout
    assert "upgrade_available=1" in environment
    assert "mender_boot_part=3" in environment
    assert "mender_boot_part_hex=0x3" in environment
    # During a Mender trial, the committed pre-update group remains the default.
    assert _boot_info(vm)["defaultGroup"] == "a"

    # Without a physical U-Boot reboot the VM is still running group A. Mender
    # deliberately reports that committed group as the default during a trial,
    # so a pre-reboot commit is a no-op and must preserve the trial variables.
    _ctrl(vm, ["system", "commit"])
    environment = vm.run(["cat", "/tmp/rugix-fwenv"], hide=True).stdout
    assert "upgrade_available=1" in environment
    assert "mender_boot_part=3" in environment
