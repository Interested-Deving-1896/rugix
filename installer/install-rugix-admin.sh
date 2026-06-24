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
RUGIX_ADMIN_ADDRESS="${RUGIX_ADMIN_ADDRESS:-0.0.0.0:8088}"
RUGIX_ADMIN_PORT="${RUGIX_ADMIN_PORT:-${RUGIX_ADMIN_ADDRESS##*:}}"
RUGIX_ADMIN_FIREWALL_ZONE="${RUGIX_ADMIN_FIREWALL_ZONE:-}"

case "$(uname -m)" in
    x86_64|amd64) RUGIX_TARGET="x86_64-unknown-linux-musl" ;;
    aarch64|arm64) RUGIX_TARGET="aarch64-unknown-linux-musl" ;;
    armv7l|armv8l) RUGIX_TARGET="armv7-unknown-linux-musleabihf" ;;
    arm*) RUGIX_TARGET="arm-unknown-linux-musleabihf" ;;
    *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

if ! [[ "${RUGIX_ADMIN_PORT}" =~ ^[0-9]+$ ]] \
    || ((RUGIX_ADMIN_PORT < 1 || RUGIX_ADMIN_PORT > 65535)); then
    echo "invalid Rugix Admin port: ${RUGIX_ADMIN_PORT}" >&2
    exit 1
fi

if [[ "${RUGIX_ADMIN_ADDRESS}" == *$'\n'* || "${RUGIX_ADMIN_ADDRESS}" == *\"* ]]; then
    echo "invalid Rugix Admin address: ${RUGIX_ADMIN_ADDRESS}" >&2
    exit 1
fi

apt-get update
apt-get install -y ca-certificates curl jq tar

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

RUGIX_VERSION="$(resolve_rugix_version "${RUGIX_VERSION}")"
if [[ -z "${RUGIX_VERSION}" || "${RUGIX_VERSION}" == "null" ]]; then
    echo "unable to resolve Rugix release version" >&2
    exit 1
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT
archive="${tmpdir}/binaries.tar"
url="https://github.com/${GITHUB_REPO}/releases/download/${RUGIX_VERSION}/binaries-${RUGIX_TARGET}.tar"
echo "downloading ${url}"
curl -fL "${url}" -o "${archive}"
tar -xf "${archive}" -C "${tmpdir}"
install -m 755 "${tmpdir}/rugix-admin" /usr/bin/rugix-admin

cat >/etc/systemd/system/rugix-admin.service <<EOF
[Unit]
Description=Rugix Admin
ConditionFileIsExecutable=/usr/bin/rugix-admin

[Service]
ExecStart=/usr/bin/rugix-admin --address ${RUGIX_ADMIN_ADDRESS}
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now rugix-admin.service

if command -v firewall-cmd >/dev/null 2>&1; then
    firewall_args=(--permanent)
    if [[ -n "${RUGIX_ADMIN_FIREWALL_ZONE}" ]]; then
        firewall_args+=(--zone "${RUGIX_ADMIN_FIREWALL_ZONE}")
    fi
    firewall_args+=(--add-port "${RUGIX_ADMIN_PORT}/tcp")

    if firewall-cmd --state >/dev/null 2>&1; then
        firewall-cmd "${firewall_args[@]}"
        firewall-cmd --reload
    else
        echo "firewall-cmd is available, but firewalld is not running; skipping firewall rule" >&2
    fi
fi

if ! command -v rugix-ctrl >/dev/null 2>&1; then
    echo "warning: rugix-ctrl was not found; Rugix Admin requires rugix-ctrl to manage the system" >&2
fi

cat <<EOF

Rugix Admin is installed and listening on ${RUGIX_ADMIN_ADDRESS}.

Next steps:
  Open Rugix Admin:
    http://<device-address>:${RUGIX_ADMIN_PORT}/

  Check the service:
    systemctl status rugix-admin.service

  Follow logs:
    journalctl -u rugix-admin.service -f
EOF
