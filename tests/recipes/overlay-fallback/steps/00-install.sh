#!/usr/bin/env bash

set -euo pipefail

mkdir -p /etc/rugix /usr/local/sbin
cat >/etc/rugix/state.toml <<'EOF'
overlay = "discard"
overlay-fallback = "in-memory"
EOF

cat >/usr/local/sbin/mount <<'EOF'
#!/bin/sh

for arg in "$@"; do
    if [ "$arg" = "/run/rugix/mounts/data/overlay/root" ]; then
        echo "simulated persistent overlay mount failure" >&2
        exit 42
    fi
done

exec /usr/bin/mount "$@"
EOF
chmod +x /usr/local/sbin/mount
