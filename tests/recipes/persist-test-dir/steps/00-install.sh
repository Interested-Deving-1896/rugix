#!/bin/bash

set -euo pipefail

mkdir -p /etc/rugix/state /var/lib/rugix-test
cat >/etc/rugix/state/rugix-test.toml <<'TOML'
[[persist]]
directory = "/var/lib/rugix-test"
TOML
echo factory >/var/lib/rugix-test/default
