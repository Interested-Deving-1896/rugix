"""Fixtures for the Rugix Ctrl and Rugix Bundler system tests."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest
from rugix_testkit import Parameters
from rugix_testkit.locks import exclusive_lock

REPOSITORY_DIR = Path(__file__).resolve().parent.parent


def pytest_configure(config: pytest.Config) -> None:
    # Bakery installs Rugix Ctrl from these binaries and the tests run Bundler
    # from them.
    binaries = REPOSITORY_DIR / "build" / "binaries"
    if binaries.is_dir():
        os.environ.setdefault("RUGIX_BINARIES_DIR", str(binaries))


@pytest.fixture(scope="session")
def update_keys(rugix_project: Path) -> Path:
    """Directory with update certificates and the signer's key.

    ``root.crt`` issues ``signer.crt``. ``other-root.crt`` is an unrelated root.
    """
    keys = rugix_project / "keys"
    with exclusive_lock(keys / ".lock"):
        if not (keys / "signer.crt").exists():
            _generate_update_keys(keys)
    return keys


@pytest.fixture(scope="session")
def bakery_parameters(update_keys: Path) -> Parameters:
    return {
        # Install the Rugix Ctrl build under test in every layer.
        "core/rugix-ctrl": {"version": "container"},
        "install-root-certs": {
            "root_crt": (update_keys / "root.crt").read_text(),
            "other_root_crt": (update_keys / "other-root.crt").read_text(),
        }
    }


def _generate_update_keys(keys: Path) -> None:
    keys.mkdir(parents=True, exist_ok=True)
    for root in ("root", "other-root"):
        _openssl(
            keys,
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
            f"{root}.key",
        )
        _openssl(
            keys,
            "req",
            "-x509",
            "-new",
            "-key",
            f"{root}.key",
            "-out",
            f"{root}.crt",
            "-days",
            "3650",
            "-subj",
            "/CN=Update CA",
        )
    _openssl(
        keys,
        "ecparam",
        "-name",
        "prime256v1",
        "-genkey",
        "-noout",
        "-out",
        "signer.key",
    )
    _openssl(
        keys,
        "req",
        "-new",
        "-key",
        "signer.key",
        "-out",
        "signer.csr",
        "-subj",
        "/CN=Update Signer",
    )
    (keys / "signer.ext").write_text(
        "basicConstraints=CA:FALSE\nkeyUsage=digitalSignature\n"
    )
    _openssl(
        keys,
        "x509",
        "-req",
        "-in",
        "signer.csr",
        "-CA",
        "root.crt",
        "-CAkey",
        "root.key",
        "-CAcreateserial",
        "-out",
        "signer.crt",
        "-days",
        "3650",
        "-extfile",
        "signer.ext",
    )


def _openssl(keys: Path, *args: str) -> None:
    subprocess.run(["openssl", *args], cwd=keys, check=True, capture_output=True)
