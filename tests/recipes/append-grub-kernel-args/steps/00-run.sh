#!/bin/bash

set -euo pipefail

GRUB_CFG="${RUGIX_LAYER_DIR}/roots/boot/grub.cfg"

if [ -n "${RECIPE_PARAM_ARGS}" ]; then
    sed -i "s|^linux /vmlinuz \${rugpi_bootargs}$|linux /vmlinuz \${rugpi_bootargs} ${RECIPE_PARAM_ARGS}|" "${GRUB_CFG}"
fi
