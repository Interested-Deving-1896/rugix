"""End-to-end test for quiet Rugix initialization."""

from rugix_testkit import BakeryBuilder, QemuVM, amd64_efi_config


def test_quiet_init_suppresses_routine_output(bakery: BakeryBuilder) -> None:
    """Test that quiet init retains errors while suppressing routine output."""
    image = bakery.bake_image("init-quiet")
    qemu = QemuVM(amd64_efi_config(image))
    qemu.prepare()
    qemu.start()
    try:
        output = qemu.wait_for_serial("error during initialization", timeout=180)
    finally:
        qemu.stop()
        qemu.cleanup()

    assert "error during initialization" in output
    assert 'running hooks for "boot/pre-init"' not in output
    assert "Setting up bind mounts" not in output
    assert "Pivoting root mount point" not in output
    assert "Starting system init process" not in output
