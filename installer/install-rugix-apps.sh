#!/usr/bin/env bash
set -euo pipefail

if [[ "$(id -u)" -ne 0 ]]; then
    echo "run as root, for example: sudo bash $0" >&2
    exit 1
fi

if ! command -v apt-get >/dev/null 2>&1; then
    distro="unknown"
    if [[ -r /etc/os-release ]]; then
        . /etc/os-release
        distro="${PRETTY_NAME:-${ID:-unknown}}"
    fi
    echo "unsupported distro: ${distro}; this installer requires apt-get" >&2
    exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
    echo "this installer requires systemd" >&2
    exit 1
fi

GITHUB_REPO="${RUGIX_GITHUB_REPO:-rugix/rugix}"
RUGIX_VERSION="${RUGIX_VERSION:-v1}"
RUGIX_DEB_VARIANT="${RUGIX_DEB_VARIANT:-musl}"

case "$(uname -m)" in
    x86_64|amd64) DEB_ARCH="amd64" ;;
    aarch64|arm64) DEB_ARCH="arm64" ;;
    armv7l|armv8l) DEB_ARCH="armhf" ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

case "${RUGIX_DEB_VARIANT}" in
    musl|gnu) ;;
    *) echo "unsupported Debian package variant: ${RUGIX_DEB_VARIANT}" >&2; exit 1 ;;
esac

apt-get update
apt-get install -y ca-certificates curl jq

if ! command -v docker >/dev/null 2>&1; then
    curl -fsSL https://test.docker.com -o /tmp/install-docker.sh
    sh /tmp/install-docker.sh
fi

systemctl enable --now docker.service
docker compose version >/dev/null

resolve_rugix_version() {
    local requested="$1"
    local api="https://api.github.com/repos/${GITHUB_REPO}/releases"
    if [[ "${requested}" == "latest" ]]; then
        curl -fsSL "${api}?per_page=100" \
            | jq -r \
                '[.[] | select((.draft | not) and (.prerelease | not))]
                 | sort_by(.published_at)
                 | last
                 | .tag_name'
    elif [[ "${requested}" =~ ^v[0-9]+$ ]]; then
        curl -fsSL "${api}?per_page=100" \
            | jq -r --arg prefix "${requested}." \
                '[.[] | select((.draft | not) and (.prerelease | not) and (.tag_name | startswith($prefix)))]
                 | sort_by(.published_at)
                 | last
                 | .tag_name'
    else
        echo "${requested}"
    fi
}

release_tag_to_deb_version() {
    local version="${1#v}"
    if [[ "${version}" == *-* ]]; then
        local base="${version%%-*}"
        local rest="${version#*-}"
        version="${base}+${rest//-/.}"
    fi
    echo "${version}"
}

RUGIX_VERSION="$(resolve_rugix_version "${RUGIX_VERSION}")"
if [[ -z "${RUGIX_VERSION}" || "${RUGIX_VERSION}" == "null" ]]; then
    echo "unable to resolve Rugix release version" >&2
    exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT
deb_version="$(release_tag_to_deb_version "${RUGIX_VERSION}")"
package="rugix-ctrl-${RUGIX_DEB_VARIANT}"
deb="${tmpdir}/${package}_${deb_version}_${DEB_ARCH}.deb"
url="https://github.com/${GITHUB_REPO}/releases/download/${RUGIX_VERSION}/${package}_${deb_version}_${DEB_ARCH}.deb"
echo "downloading ${url}"
curl -fL "${url}" -o "${deb}"
apt-get install -y "${deb}"

cat >/etc/systemd/system/rugix-apps-restore-units.service <<'EOF'
[Unit]
Description=Restore Rugix app units into systemd
After=local-fs.target
DefaultDependencies=no

[Service]
Type=oneshot
ExecStart=/usr/bin/rugix-ctrl apps service-manager systemd restore-units
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF

cat >/etc/systemd/system/rugix-apps-recover.service <<'EOF'
[Unit]
Description=Recover interrupted Rugix app transitions
After=multi-user.target rugix-apps-restore-units.service docker.service
Wants=rugix-apps-restore-units.service

[Service]
Type=oneshot
ExecStart=/usr/bin/rugix-ctrl apps recover
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now rugix-apps-restore-units.service
systemctl enable --now rugix-apps-recover.service

rugix-ctrl --version
cat <<'EOF'

Rugix Ctrl Apps runtime is installed.

Next steps:
  Install a Rugix app bundle:
    rugix-ctrl apps install --bundle-hash "$(cat app.rugixb-hash)" app.rugixb

  Inspect installed apps:
    rugix-ctrl apps list
EOF
