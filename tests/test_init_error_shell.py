"""End-to-end tests for Rugix init error shell handling."""

from __future__ import annotations

from pathlib import Path

from rugix_testkit import BakeryBuilder, QemuVM, amd64_efi_config


def test_init_error_without_shell_prints_opt_in_hint(bakery: BakeryBuilder) -> None:
    image = bakery.bake_image("init-error-shell-disabled")
    output = _boot_until_serial(
        image,
        "The init error shell is disabled",
        timeout=180,
    )

    assert "rugix.init.shell_on_error" in output
    assert "Press any key" not in output


def test_init_error_shell_starts_after_keypress(bakery: BakeryBuilder) -> None:
    image = bakery.bake_image("init-error-shell-enabled")
    qemu = QemuVM(amd64_efi_config(image))
    qemu.prepare()
    qemu.start()
    try:
        qemu.wait_for_serial("Press any key within", timeout=180)
        qemu.write_serial(b"x")
        qemu.wait_for_serial("Starting debug shell.", timeout=30)

        qemu.write_serial(b"echo RUGIX_INIT_ERROR_SHELL_READY\n")
        output = qemu.wait_for_serial("RUGIX_INIT_ERROR_SHELL_READY", timeout=30)
    finally:
        qemu.stop()
        qemu.cleanup()

    assert "Starting debug shell." in output


def _boot_until_serial(image: Path, text: str, *, timeout: float) -> str:
    qemu = QemuVM(amd64_efi_config(image))
    qemu.prepare()
    qemu.start()
    try:
        return qemu.wait_for_serial(text, timeout=timeout)
    finally:
        qemu.stop()
        qemu.cleanup()
