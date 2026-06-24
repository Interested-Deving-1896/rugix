# Rugix Installers

This directory contains standalone installer scripts for integrating Rugix
components into existing Linux systems.

## Rugix Apps

`install-rugix-apps.sh` installs:

- Docker, if it is not already available.
- `rugix-ctrl` from a Rugix GitHub release Debian package.
- Systemd units required by Rugix Apps:
  - `rugix-apps-restore-units.service`
  - `rugix-apps-recover.service`

Run it as root on an apt-based system with systemd, such as Debian or Ubuntu:

```sh
sudo bash installer/install-rugix-apps.sh
```

By default, `RUGIX_VERSION=v1` resolves to the latest stable `v1.x` release
from `rugix/rugix`. `latest` and `vN` selectors ignore prereleases. Set
`RUGIX_VERSION` to an exact tag to install that tag, including prerelease tags:

```sh
sudo RUGIX_VERSION=latest bash installer/install-rugix-apps.sh
sudo RUGIX_VERSION=v1.2.0 bash installer/install-rugix-apps.sh
sudo RUGIX_GITHUB_REPO=my-org/rugix RUGIX_VERSION=v1.2.0 bash installer/install-rugix-apps.sh
```

The installer uses the `rugix-ctrl-musl` Debian package by default. Set
`RUGIX_DEB_VARIANT=gnu` to install `rugix-ctrl-gnu` instead. The Rugix Ctrl
package only provides the `rugix-ctrl` binary; the app recovery systemd units
are installed by this script.

## Rugix Admin

`install-rugix-admin.sh` installs:

- `rugix-admin` from a Rugix GitHub release binary tarball.
- `rugix-admin.service`, enabled and started through systemd.
- A firewalld rule for the Rugix Admin TCP port if `firewall-cmd` is available
  and firewalld is running.

Run it as root on an apt-based system with systemd, such as Debian or Ubuntu:

```sh
sudo bash installer/install-rugix-admin.sh
```

By default, Rugix Admin listens on `0.0.0.0:8088` and `RUGIX_VERSION=v1`
resolves to the latest stable `v1.x` release. `latest` and `vN` selectors
ignore prereleases. Override the release, address, or firewalld zone if needed:

```sh
sudo RUGIX_VERSION=v1.2.0 bash installer/install-rugix-admin.sh
sudo RUGIX_ADMIN_ADDRESS=0.0.0.0:8088 bash installer/install-rugix-admin.sh
sudo RUGIX_ADMIN_FIREWALL_ZONE=public bash installer/install-rugix-admin.sh
```

`rugix-admin` calls `rugix-ctrl` for system and app operations. Install Rugix
Ctrl first when the target system does not already provide it.
