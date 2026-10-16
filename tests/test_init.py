"""Rugix Ctrl as init reports errors on the serial console.

The systems of these tests have an invalid ``/etc/rugix/system.toml``, so
initialization fails after printing its routine output.
"""

from __future__ import annotations

from collections.abc import Callable

from rugix_testkit import BakeryBuilder, QemuVM, VMConfig, amd64_efi_config

ERROR = "error during initialization"
ROUTINE_OUTPUT = ["by Silitics", "PATH='", 'running hooks for "boot/pre-init"']


def test_init_error_without_shell_prints_opt_in_hint(
    bakery: BakeryBuilder, start_qemu: Callable[[VMConfig], QemuVM]
) -> None:
    """Without the error shell, init explains how to enable it."""
    qemu = start_qemu(amd64_efi_config(bakery.bake_image("init-error-shell-disabled")))
    output = qemu.wait_for_serial("set on the kernel cmdline", timeout=180)

    assert "rugix.init.shell_on_error" in output
    assert "Press any key" not in output
    for routine in ROUTINE_OUTPUT:
        assert routine in output


def test_init_error_shell_starts_after_keypress(
    bakery: BakeryBuilder, start_qemu: Callable[[VMConfig], QemuVM]
) -> None:
    """With the error shell enabled, a keypress opens a working shell."""
    qemu = start_qemu(amd64_efi_config(bakery.bake_image("init-error-shell-enabled")))
    qemu.wait_for_serial("Press any key within", timeout=180)
    qemu.write_serial(b"x")
    qemu.wait_for_serial("Starting debug shell.", timeout=30)
    # The terminal echoes the command, so only the shell can print 42.
    qemu.write_serial(b"echo SHELL_$((6 * 7))\n")
    qemu.wait_for_serial("SHELL_42", timeout=30)


def test_quiet_init_suppresses_routine_output(
    bakery: BakeryBuilder, start_qemu: Callable[[VMConfig], QemuVM]
) -> None:
    """With ``rugix.init.quiet``, init prints errors but no routine output."""
    qemu = start_qemu(amd64_efi_config(bakery.bake_image("init-quiet")))
    output = qemu.wait_for_serial(ERROR, timeout=180)

    for routine in ROUTINE_OUTPUT:
        assert routine not in output
