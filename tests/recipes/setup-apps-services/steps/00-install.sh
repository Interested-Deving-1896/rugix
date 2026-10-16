#!/bin/bash

set -euo pipefail

install -D -m 644 "${RECIPE_DIR}/files/rugix-apps-restore-units.service" -t /usr/lib/systemd/system/
install -D -m 644 "${RECIPE_DIR}/files/rugix-apps-recover.service" -t /usr/lib/systemd/system/
install -D -m 644 "${RECIPE_DIR}/files/rugix-apps-gc.service" -t /usr/lib/systemd/system/
install -D -m 644 "${RECIPE_DIR}/files/rugix-apps-gc.timer" -t /usr/lib/systemd/system/

systemctl enable rugix-apps-restore-units
systemctl enable rugix-apps-recover
systemctl enable rugix-apps-gc.timer
