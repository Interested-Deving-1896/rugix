#!/bin/bash

set -euo pipefail

mkdir -p /etc/rugix
cat >/etc/rugix/system.toml <<'EOF'
[
EOF
