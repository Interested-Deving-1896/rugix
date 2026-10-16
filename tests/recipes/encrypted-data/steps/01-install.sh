#!/bin/bash

set -euo pipefail

mkdir -p /etc/rugix
install -m 0644 "${RECIPE_DIR}/files/system.toml" /etc/rugix/system.toml
install -m 0400 "${RECIPE_DIR}/files/luks-data.key" /etc/rugix/luks-data.key
