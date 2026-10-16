"""The LUKS2 passphrase driver encrypts the data partition."""

from __future__ import annotations

import pytest
from rugix_testkit import VMHandle

DATA_PARTITION = "/dev/vda6"


@pytest.fixture
def boot_timeout() -> float:
    # The first boot formats the encrypted data partition, which takes a while.
    return 600.0


@pytest.fixture
def boot_system() -> str:
    return "customized-amd64-luks2-passphrase"


def test_encrypted_data_persists_across_reboot(vm: VMHandle) -> None:
    """The data partition is mounted from a LUKS2 volume that opens again
    after a reboot with its data intact."""
    assert _mapper_type(vm) == "LUKS2"
    uuid = _luks_uuid(vm)
    vm.run(["touch", "/run/rugix/state/marker"], hide=True)

    vm.reboot()

    vm.run(["test", "-e", "/run/rugix/state/marker"], hide=True)
    assert _mapper_type(vm) == "LUKS2"
    assert _luks_uuid(vm) == uuid


def test_data_wipe_recreates_volume(vm: VMHandle) -> None:
    """``rugix-ctrl data wipe`` replaces the volume with an empty one."""
    uuid = _luks_uuid(vm)
    vm.run(["touch", "/run/rugix/state/marker"], hide=True)

    vm.run_disconnecting(["rugix-ctrl", "data", "wipe", "--yes"])
    vm.wait_for_reboot()

    assert not _exists(vm, "/run/rugix/state/marker")
    assert _mapper_type(vm) == "LUKS2"
    assert _luks_uuid(vm) != uuid


def test_state_reset_keeps_volume(vm: VMHandle) -> None:
    """``rugix-ctrl state reset`` clears the state but keeps the volume and
    data outside the state profile."""
    uuid = _luks_uuid(vm)
    vm.run(
        ["touch", "/run/rugix/state/marker", "/run/rugix/mounts/data/kept"], hide=True
    )

    vm.run_disconnecting(["rugix-ctrl", "state", "reset"])
    vm.wait_for_reboot()

    assert not _exists(vm, "/run/rugix/state/marker")
    vm.run(["test", "-e", "/run/rugix/mounts/data/kept"], hide=True)
    assert _luks_uuid(vm) == uuid


def _mapper_type(vm: VMHandle) -> str:
    """Return the dm-crypt type of the device mounted as the data partition."""
    source = vm.run(["findmnt", "-no", "SOURCE", "/run/rugix/mounts/data"], hide=True)
    mapper = source.stdout.strip().removeprefix("/dev/mapper/")
    status = vm.run(["cryptsetup", "status", mapper], hide=True).stdout
    return next(
        line.split(":", 1)[1].strip()
        for line in status.splitlines()
        if line.strip().startswith("type:")
    )


def _luks_uuid(vm: VMHandle) -> str:
    return vm.run(["cryptsetup", "luksUUID", DATA_PARTITION], hide=True).stdout.strip()


def _exists(vm: VMHandle, path: str) -> bool:
    return vm.run(["test", "-e", path], check=False, hide=True).ok
