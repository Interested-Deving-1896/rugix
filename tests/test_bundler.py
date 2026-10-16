"""Pack app configuration with the selected host binary despite an older PATH binary."""

from __future__ import annotations

import json
import os
import platform
import shutil
import tarfile
from pathlib import Path

import pytest
from rugix_testkit import Bundler


@pytest.mark.parametrize("selection", ["override", "binaries"])
def test_host_app_configuration(
    bundler: Bundler,
    rugix_project: Path,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    selection: str,
) -> None:
    """Pack configuration with the selected build despite an older PATH Bundler."""
    executable = shutil.which(bundler.executable)
    assert executable is not None
    stale_bin = tmp_path / "stale-bin"
    stale_bin.mkdir()
    stale_bundler = stale_bin / "rugix-bundler"
    stale_bundler.write_text("#!/bin/sh\nexit 99\n")
    stale_bundler.chmod(0o755)
    monkeypatch.setenv("PATH", f"{stale_bin}:{os.environ['PATH']}")
    binaries_dir = tmp_path / "binaries"
    monkeypatch.setenv("RUGIX_BINARIES_DIR", str(binaries_dir))
    if selection == "override":
        monkeypatch.setenv("RUGIX_BUNDLER", executable)
    else:
        monkeypatch.delenv("RUGIX_BUNDLER", raising=False)
        target_dir = binaries_dir / "x86_64-unknown-linux-musl"
        target_dir.mkdir(parents=True)
        (target_dir / "rugix-bundler").symlink_to(executable)
        monkeypatch.setattr(platform, "machine", lambda: "x86_64")
        monkeypatch.setattr(platform, "system", lambda: "Linux")

    host_bundler = Bundler(tmp_path)
    app_dir = rugix_project / "apps" / "generic-config"
    output = tmp_path / "configuration.rugixb"
    host_bundler.apps_pack(
        "generic",
        [
            "--app",
            "hello-config",
            "--config-schema",
            str(app_dir / "config.schema.json"),
            "--config-default",
            str(app_dir / "config.default.json"),
        ],
        app_dir / "orchestrator",
        output,
    )
    unpacked = tmp_path / "unpacked"
    host_bundler.run(["unpack", str(output), str(unpacked)])
    with tarfile.open(unpacked / "payloads" / "base.tar") as archive:
        for name in ("config.schema.json", "config.default.json"):
            bundled_file = archive.extractfile(name)
            assert bundled_file is not None
            assert json.load(bundled_file) == json.loads((app_dir / name).read_text())
