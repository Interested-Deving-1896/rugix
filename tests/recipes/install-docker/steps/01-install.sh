#!/bin/bash

set -euo pipefail

curl -fsSL https://test.docker.com -o /tmp/install-docker.sh
sh /tmp/install-docker.sh

mkdir -p /etc/rugix/state
cat >/etc/rugix/state/docker.toml<<EOF
[[persist]]
directory="/var/lib/containerd"
[[persist]]
directory="/var/lib/docker"
EOF

systemctl enable docker