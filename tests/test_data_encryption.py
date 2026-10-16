"""End-to-end coverage for the LUKS2 passphrase data partition driver."""

from __future__ import annotations

import pytest
from rugix_testkit import VMHandle


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-luks2-passphrase"


def _data_block_device(vm: VMHandle) -> str:
    """Return the block device path backing ``/run/rugix/mounts/data``."""
    res = vm.run(["findmnt", "-no", "SOURCE", "/run/rugix/mounts/data"], hide=True)
    return res.stdout.strip()


def _luks_uuid(vm: VMHandle, device: str) -> str:
    return vm.run(["cryptsetup", "luksUUID", device], hide=True).stdout.strip()


def test_data_partition_is_luks_encrypted(vm: VMHandle) -> None:
    """The bare partition device should report a LUKS2 superblock; the
    mounted filesystem should live on the dm-crypt mapper."""
    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition should be mounted via dm-crypt mapper, got {mapper_path!r}"
    )

    status = vm.run(
        ["cryptsetup", "status", mapper_path.removeprefix("/dev/mapper/")],
        hide=True,
    ).stdout
    assert "type:    LUKS2" in status, f"unexpected cryptsetup status:\n{status}"

    backing = next(
        line.split(None, 1)[1].strip()
        for line in status.splitlines()
        if line.strip().startswith("device:")
    )
    is_luks = vm.run(["cryptsetup", "isLuks", backing], check=False, hide=True)
    assert is_luks.ok, f"{backing} should report as LUKS, got rc={is_luks.return_code}"


def test_persistence_across_reboot(vm: VMHandle) -> None:
    """State survives a reboot — proving the LUKS volume re-opens
    correctly, not just on the bootstrap boot."""
    vm.run(["touch", "/run/rugix/state/encrypted-persistence-marker"], hide=True)

    vm.reboot()

    vm.run(["test", "-e", "/run/rugix/state/encrypted-persistence-marker"], hide=True)
    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition mapper missing after reboot, got {mapper_path!r}"
    )


def test_data_wipe_destroys_state_and_reformats(vm: VMHandle) -> None:
    """``rugix-ctrl data wipe`` should cryptographically erase the LUKS
    header, reformat with a fresh master key, and clear all state."""
    vm.run(["touch", "/run/rugix/state/should-not-survive-wipe"], hide=True)
    pre_wipe_uuid = _luks_uuid(vm, "/dev/vda6")
    assert pre_wipe_uuid, "couldn't read pre-wipe LUKS UUID"

    vm.run_disconnecting(["rugix-ctrl", "data", "wipe", "--yes"])
    vm.wait_for_reboot()

    res = vm.run(
        ["test", "-e", "/run/rugix/state/should-not-survive-wipe"],
        check=False,
        hide=True,
    )
    assert not res.ok, "data wipe should remove all state from the data partition"

    post_wipe_uuid = _luks_uuid(vm, "/dev/vda6")
    assert post_wipe_uuid, "couldn't read post-wipe LUKS UUID"
    assert post_wipe_uuid != pre_wipe_uuid, (
        f"data wipe should rotate the LUKS master key (UUID was {pre_wipe_uuid!r}, "
        f"is still {post_wipe_uuid!r})"
    )

    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition mapper missing after wipe, got {mapper_path!r}"
    )


def test_state_reset_clears_profile_only(vm: VMHandle) -> None:
    """``state reset`` should clear the active profile but leave the LUKS
    volume intact."""
    vm.run(["touch", "/run/rugix/state/should-not-survive-reset"], hide=True)

    vm.run_disconnecting(["rugix-ctrl", "state", "reset"])
    vm.wait_for_reboot()

    res = vm.run(
        ["test", "-e", "/run/rugix/state/should-not-survive-reset"],
        check=False,
        hide=True,
    )
    assert not res.ok, "state reset should remove files from the active profile"

    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        "state reset should not destroy the LUKS volume"
    )
