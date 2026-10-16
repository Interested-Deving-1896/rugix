"""Rugix Ctrl as init reports errors on the serial console.

The systems of these tests have an invalid ``/etc/rugix/system.toml``, so
initialization fails after printing its routine output.
"""

from __future__ import annotations

from pathlib import Path

from rugix_testkit import BakeryBuilder, QemuVM, amd64_efi_config

ERROR = "error during initialization"
ROUTINE_OUTPUT = ["by Silitics", "PATH='", 'running hooks for "boot/pre-init"']


def test_init_error_without_shell_prints_opt_in_hint(bakery: BakeryBuilder) -> None:
    """Without the error shell, init explains how to enable it."""
    output = _boot_until(
        bakery.bake_image("init-error-shell-disabled"),
        "The init error shell is disabled",
    )

    assert "rugix.init.shell_on_error" in output
    assert "Press any key" not in output
    for routine in ROUTINE_OUTPUT:
        assert routine in output


def test_init_error_shell_starts_after_keypress(bakery: BakeryBuilder) -> None:
    """With the error shell enabled, a keypress opens a working shell."""
    qemu = QemuVM(amd64_efi_config(bakery.bake_image("init-error-shell-enabled")))
    qemu.prepare()
    qemu.start()
    try:
        qemu.wait_for_serial("Press any key within", timeout=180)
        qemu.write_serial(b"x")
        qemu.wait_for_serial("Starting debug shell.", timeout=30)
        # The terminal echoes the command, so only the shell can print 42.
        qemu.write_serial(b"echo SHELL_$((6 * 7))\n")
        qemu.wait_for_serial("SHELL_42", timeout=30)
    finally:
        qemu.stop()
        qemu.cleanup()


def test_quiet_init_suppresses_routine_output(bakery: BakeryBuilder) -> None:
    """With ``rugix.init.quiet``, init prints errors but no routine output."""
    output = _boot_until(bakery.bake_image("init-quiet"), ERROR)

    for routine in ROUTINE_OUTPUT:
        assert routine not in output


def _boot_until(image: Path, text: str) -> str:
    qemu = QemuVM(amd64_efi_config(image))
    qemu.prepare()
    qemu.start()
    try:
        return qemu.wait_for_serial(text, timeout=180)
    finally:
        qemu.stop()
        qemu.cleanup()
