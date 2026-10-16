"""The LUKS2 TPM2 driver encrypts the data partition with a TPM-sealed key.

Each test boots with a fresh swtpm-emulated TPM 2.0 device.
"""

from __future__ import annotations

from collections.abc import Callable, Generator
from pathlib import Path

import pytest
from rugix_testkit import (
    BakeryBuilder,
    RugixCtrl,
    SwtpmProcess,
    VMConfig,
    VMHandle,
    amd64_efi_config,
)

DATA_PARTITION = "/dev/vda6"


@pytest.fixture
def boot_timeout() -> float:
    # The first boot formats the encrypted data partition, which takes a while.
    return 600.0


@pytest.fixture
def swtpm() -> Generator[SwtpmProcess]:
    with SwtpmProcess() as swtpm:
        yield swtpm


@pytest.fixture
def vm(
    swtpm: SwtpmProcess,
    bakery: BakeryBuilder,
    start_vm: Callable[[VMConfig], VMHandle],
) -> VMHandle:
    image = bakery.bake_image("customized-amd64-luks2-tpm2")
    return start_vm(amd64_efi_config(image, tpm_socket=swtpm.socket_path))


def test_tpm_unlocks_data_across_update(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """The first boot seals the volume key to the TPM. The key unlocks the data
    partition again after booting an updated system."""
    assert "systemd-tpm2" in _luks_dump(vm)
    vm.run(["touch", "/run/rugix/state/marker"], hide=True)

    rugix.install_and_reboot(
        bundle_url(bakery.bake_bundle("customized-amd64-luks2-tpm2"))
    )

    rugix.assert_boot(default="a", active="b")
    vm.run(["test", "-e", "/run/rugix/state/marker"], hide=True)
    source = vm.run(["findmnt", "-no", "SOURCE", "/run/rugix/mounts/data"], hide=True)
    assert source.stdout.startswith("/dev/mapper/")


def test_data_wipe_enrolls_new_key(vm: VMHandle) -> None:
    """``rugix-ctrl data wipe`` recreates the volume and seals its new key."""
    uuid = _luks_uuid(vm)
    vm.run(["touch", "/run/rugix/state/marker"], hide=True)

    vm.run_disconnecting(["rugix-ctrl", "data", "wipe", "--yes"])
    vm.wait_for_reboot()

    assert not vm.run(
        ["test", "-e", "/run/rugix/state/marker"], check=False, hide=True
    ).ok
    assert _luks_uuid(vm) != uuid
    assert "systemd-tpm2" in _luks_dump(vm)


def _luks_dump(vm: VMHandle) -> str:
    return vm.run(["cryptsetup", "luksDump", DATA_PARTITION], hide=True).stdout


def _luks_uuid(vm: VMHandle) -> str:
    return vm.run(["cryptsetup", "luksUUID", DATA_PARTITION], hide=True).stdout.strip()
