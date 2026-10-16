#!/bin/bash

set -euo pipefail

mkdir -p /etc/rugix
printf '%s' "${RECIPE_PARAM_ROOT_CRT}" >/etc/rugix/root.crt
printf '%s' "${RECIPE_PARAM_OTHER_ROOT_CRT}" >/etc/rugix/other-root.crt
