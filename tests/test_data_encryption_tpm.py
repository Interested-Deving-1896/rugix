"""End-to-end coverage for the LUKS2 TPM2 data partition driver.

Boots an image with a swtpm-emulated TPM 2.0 device. Each test starts a
fresh swtpm so they don't share TPM state.
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


def _data_block_device(vm: VMHandle) -> str:
    res = vm.run(["findmnt", "-no", "SOURCE", "/run/rugix/mounts/data"], hide=True)
    return res.stdout.strip()


def _backing_device(vm: VMHandle, mapper_path: str) -> str:
    status = vm.run(
        ["cryptsetup", "status", mapper_path.removeprefix("/dev/mapper/")],
        hide=True,
    ).stdout
    return next(
        line.split(None, 1)[1].strip()
        for line in status.splitlines()
        if line.strip().startswith("device:")
    )


def _luks_uuid(vm: VMHandle, device: str) -> str:
    return vm.run(["cryptsetup", "luksUUID", device], hide=True).stdout.strip()


def test_tpm_device_is_present(vm: VMHandle) -> None:
    res = vm.run(["test", "-c", "/dev/tpm0"], hide=True, check=False)
    assert res.ok, "guest should expose /dev/tpm0 from the emulated TPM"
    pcrs = vm.run(["tpm2_pcrread", "sha256:0,7"], hide=True, check=False)
    assert pcrs.ok, f"tpm2_pcrread failed: {pcrs.stderr or pcrs.stdout}"


def test_data_partition_is_tpm_backed(vm: VMHandle) -> None:
    """The mount runs through the dm-crypt mapper; the underlying partition
    carries a LUKS2 header with a `systemd-tpm2` token (no password slot)."""
    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition should be mounted via dm-crypt mapper, got {mapper_path!r}"
    )

    # luksDump succeeds only on a LUKS volume, so its output is a
    # sufficient witness that the backing device is LUKS-formatted.
    backing = _backing_device(vm, mapper_path)
    dump = vm.run(["cryptsetup", "luksDump", backing], hide=True).stdout
    assert "systemd-tpm2" in dump, (
        f"LUKS header should carry a systemd-tpm2 token:\n{dump}"
    )


def test_persistence_across_reboot(vm: VMHandle) -> None:
    """State survives a reboot — the TPM unseal works on every boot."""
    vm.run(["touch", "/run/rugix/state/tpm-persistence-marker"], hide=True)

    vm.reboot()

    vm.run(["test", "-e", "/run/rugix/state/tpm-persistence-marker"], hide=True)
    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition mapper missing after reboot, got {mapper_path!r}"
    )


def test_data_wipe_rotates_tpm_enrollment(vm: VMHandle) -> None:
    """``rugix-ctrl data wipe`` erases the LUKS header, reformats, and
    re-enrolls the TPM with a fresh sealed key."""
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

    dump = vm.run(["cryptsetup", "luksDump", "/dev/vda6"], hide=True).stdout
    assert "systemd-tpm2" in dump, (
        f"post-wipe LUKS header should carry a systemd-tpm2 token:\n{dump}"
    )

    mapper_path = _data_block_device(vm)
    assert mapper_path.startswith("/dev/mapper/"), (
        f"data partition mapper missing after wipe, got {mapper_path!r}"
    )


def test_encryption_survives_update(
    vm: VMHandle,
    rugix: RugixCtrl,
    bakery: BakeryBuilder,
    bundle_url: Callable[[Path], str],
) -> None:
    """PCR 7 binding survives an A/B update: the same sealed key that
    bootstrap enrolled keeps unlocking the data partition after we install
    a fresh image to the spare slot and reboot into it."""
    initial_mapper = _data_block_device(vm)
    assert initial_mapper.startswith("/dev/mapper/"), initial_mapper

    vm.run(["touch", "/run/rugix/state/tpm-update-marker"], hide=True)

    bundle = bundle_url(bakery.bake_bundle("customized-amd64-luks2-tpm2"))
    rugix.install_and_reboot(bundle)

    vm.run(["test", "-e", "/run/rugix/state/tpm-update-marker"], hide=True)
    post_update_mapper = _data_block_device(vm)
    assert post_update_mapper.startswith("/dev/mapper/"), (
        f"data partition mapper missing after update, got {post_update_mapper!r}"
    )

    backing = _backing_device(vm, post_update_mapper)
    dump = vm.run(["cryptsetup", "luksDump", backing], hide=True).stdout
    assert "systemd-tpm2" in dump, (
        f"LUKS header lost its TPM2 token after update:\n{dump}"
    )
