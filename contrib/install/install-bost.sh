#!/usr/bin/env bash
# Install bost-flowstation on Raspberry Pi OS / Debian arm64.
# Idempotent: aligns the source tree to origin/<branch> (reset --hard; keeps target/).
# Starts with phy_io.backend=None so the web UI comes up without an SDR;
# complete Setup in the dashboard afterward.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Aitorrio/bost-flowstation/main/contrib/install/install-bost.sh | sudo bash
#   sudo ./contrib/install/install-bost.sh
#
# Env:
#   BOST_SRC            existing checkout (default: /opt/bost-flowstation)
#   BOST_BRANCH         git branch (default: main). Use `beta` for the preview channel.
#                       Dashboard OTA channel is set to match (stable↔main, beta↔beta).
#   BOST_REPO           git URL (default: https://github.com/Aitorrio/bost-flowstation.git)
#   BOST_FORCE_CLEAN=1  delete source tree and re-clone (keeps /etc/flowstation)
#   BOST_USE_DEB=1      prefer .deb asset if available (optional)
#   BOST_SKIP_BUILD=1   skip cargo build (use existing binary)
#   BOST_SKIP_TETRA_CODEC=1  skip outerplane ACELP lib (LST voice / Asterisk feature off)
#   BOST_DASH_PORTS     standard (80→443) or high (HTTPS 8443 only); prompted on TTY if unset
set -euo pipefail

REPO_URL="${BOST_REPO:-https://github.com/Aitorrio/bost-flowstation.git}"
BRANCH="${BOST_BRANCH:-main}"
SRC_ROOT="${BOST_SRC:-/opt/bost-flowstation}"
CFG_DIR="/etc/flowstation"
CFG_PATH="${CFG_DIR}/config.toml"
BIN_PATH="/usr/local/bin/bluestation-bs"
UNIT_NAME="bluestation-bs.service"
HELPER_DST="/usr/local/sbin/bost-setup-helper.sh"
SUDOERS_DST="/etc/sudoers.d/bost-setup"
SERVICE_USER="${BOST_SERVICE_USER:-bts}"

# Dashboard channel id for [dashboard].ota_channel
OTA_CHANNEL="stable"
[[ "$BRANCH" == "beta" ]] && OTA_CHANNEL="beta"

# Dashboard listen preset: standard (80→443) or high (HTTPS 8443 only).
# Override with BOST_DASH_PORTS=standard|high. Interactive prompt when TTY and unset.
DASH_PORTS_PRESET="${BOST_DASH_PORTS:-}"
DASH_HTTP_PORT=80
DASH_HTTPS_PORT=443

choose_dashboard_ports() {
  local choice=""
  if [[ -n "$DASH_PORTS_PRESET" ]]; then
    choice="$DASH_PORTS_PRESET"
  elif [[ -t 0 ]]; then
    echo
    echo "Dashboard HTTPS ports (LST microphone requires HTTPS):"
    echo "  1) Standard — HTTP :80 redirects to HTTPS :443  (recommended dedicated Pi)"
    echo "  2) High port — HTTPS :8443 only  (when 80/443 are used by nginx/apache/etc.)"
    read -r -p "Choose [1/2] (default 1): " choice || true
    case "${choice:-1}" in
      2|high|HIGH) choice="high" ;;
      *) choice="standard" ;;
    esac
  else
    choice="standard"
  fi
  case "${choice,,}" in
    high|2)
      DASH_PORTS_PRESET="high"
      DASH_HTTP_PORT=0
      DASH_HTTPS_PORT=8443
      ;;
    *)
      DASH_PORTS_PRESET="standard"
      DASH_HTTP_PORT=80
      DASH_HTTPS_PORT=443
      ;;
  esac
  log "Dashboard ports preset: ${DASH_PORTS_PRESET} (HTTP ${DASH_HTTP_PORT}, HTTPS ${DASH_HTTPS_PORT})"
}

log() { echo "==> $*"; }
warn() { echo "WARNING: $*" >&2; }
die() { echo "ERROR: $*" >&2; exit 1; }

# Fetch with explicit refspec (creates origin/<branch>) and a few TLS/network retries.
git_fetch_branch() {
  local dir="$1" branch="$2"
  local refspec="+refs/heads/${branch}:refs/remotes/origin/${branch}"
  local attempt
  for attempt in 1 2 3; do
    if [[ "$attempt" -gt 1 ]]; then
      warn "git fetch failed — retrying in $((5 * (attempt - 1)))s (attempt ${attempt}/3)…"
      sleep $((5 * (attempt - 1)))
    fi
    if git -C "$dir" fetch origin "$refspec"; then
      return 0
    fi
  done
  return 1
}

if [[ "$(id -u)" -ne 0 ]]; then
  die "run as root (sudo ./contrib/install/install-bost.sh)"
fi

ARCH="$(uname -m)"
if [[ "$ARCH" != "aarch64" && "$ARCH" != "arm64" ]]; then
  warn "expected aarch64/arm64, found $ARCH — continuing anyway"
fi
if [[ ! -f /etc/debian_version ]]; then
  warn "non-Debian host — apt steps may fail"
fi

# Ensure a service user exists for optional non-root runs / home paths.
if ! id "$SERVICE_USER" >/dev/null 2>&1; then
  log "Creating user $SERVICE_USER"
  useradd -m -s /bin/bash "$SERVICE_USER" || true
fi

export DEBIAN_FRONTEND=noninteractive
log "Installing apt dependencies"
apt-get update -qq
apt-get install -y \
  build-essential pkg-config git curl ca-certificates python3 \
  libssl-dev cmake \
  soapysdr-tools libsoapysdr-dev \
  || apt-get install -y build-essential git curl ca-certificates python3 soapysdr-tools libsoapysdr-dev

# Lime packages when present in the distro (optional at install time).
for p in soapysdr-module-lms7 soapysdr0.8-module-lms7; do
  if apt-cache show "$p" >/dev/null 2>&1; then
    apt-get install -y "$p" || true
  fi
done

# Ettus USRP B200/B210 (UHD) when present in the distro: driver module plus the firmware and
# FPGA images UHD loads into the board at start-up. Debian's UHD looks in /usr/share/uhd/images;
# the downloader writes a versioned directory, so link it.
for p in uhd-host soapysdr0.8-module-uhd soapysdr-module-uhd; do
  if apt-cache show "$p" >/dev/null 2>&1; then
    apt-get install -y "$p" || true
  fi
done
if command -v uhd_images_downloader >/dev/null 2>&1; then
  uhd_images_downloader -t b2xx >/dev/null 2>&1 || true
  if [[ ! -d /usr/share/uhd/images ]]; then
    d="$(ls -d /usr/share/uhd/*/images 2>/dev/null | head -n1 || true)"
    [[ -n "$d" ]] && ln -s "$d" /usr/share/uhd/images || true
  fi
fi

# Rust toolchain for the service user (or root if building as root).
install_rust() {
  local user="$1"
  local home
  home="$(getent passwd "$user" | cut -d: -f6)"
  if [[ -x "$home/.cargo/bin/cargo" ]]; then
    return 0
  fi
  log "Installing rustup for $user"
  sudo -u "$user" bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable'
}

# Source tree. Align to origin/<branch> the same way dashboard OTA does (reset --hard;
# preserves target/ for incremental builds). Config under /etc/flowstation is never deleted here.
if [[ -n "${BOST_SRC:-}" ]]; then
  SRC_ROOT="$BOST_SRC"
fi

if [[ "${BOST_FORCE_CLEAN:-0}" == "1" && -e "$SRC_ROOT" ]]; then
  log "BOST_FORCE_CLEAN=1 — stopping service and removing $SRC_ROOT (config kept)"
  systemctl stop "$UNIT_NAME" 2>/dev/null || true
  rm -rf "$SRC_ROOT"
fi

if [[ -d "$SRC_ROOT/.git" ]]; then
  log "Updating $SRC_ROOT → origin/$BRANCH"
  if ! git -C "$SRC_ROOT" remote get-url origin >/dev/null 2>&1; then
    git -C "$SRC_ROOT" remote add origin "$REPO_URL"
  elif [[ "$(git -C "$SRC_ROOT" remote get-url origin)" != "$REPO_URL" ]]; then
    log "Setting origin to $REPO_URL"
    git -C "$SRC_ROOT" remote set-url origin "$REPO_URL"
  fi
  git_fetch_branch "$SRC_ROOT" "$BRANCH" \
    || die "git fetch origin/$BRANCH failed after retries (network/TLS). Re-run the installer."
  # -f: local changes (e.g. a mode bit the installer set) must not block the update; reset --hard
  # below discards them anyway.
  git -C "$SRC_ROOT" checkout -f -B "$BRANCH" "origin/$BRANCH"
  git -C "$SRC_ROOT" reset --hard "origin/$BRANCH"
  log "Source at $(git -C "$SRC_ROOT" rev-parse --short HEAD) ($(git -C "$SRC_ROOT" rev-parse --abbrev-ref HEAD))"
elif [[ -e "$SRC_ROOT" ]]; then
  die "$SRC_ROOT exists but is not a git checkout.
Move it aside or remove it, then re-run (or use BOST_FORCE_CLEAN=1)."
else
  log "Cloning $REPO_URL ($BRANCH) → $SRC_ROOT"
  mkdir -p "$(dirname "$SRC_ROOT")"
  git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$SRC_ROOT" \
    || die "git clone failed (network/TLS). Re-run the installer."
fi

chown -R "$SERVICE_USER:$SERVICE_USER" "$SRC_ROOT" || true

# Native TETRA ACELP codec (outerplane) — required for LST Dispatch voice and optional Asterisk SIP.
# Installs to /usr/local; cargo then builds with --features asterisk.
CODEC_SCRIPT="$SRC_ROOT/contrib/install/install-tetra-codec.sh"
WITH_ASTERISK_FEATURE=0
if [[ "${BOST_SKIP_TETRA_CODEC:-0}" == "1" ]]; then
  warn "BOST_SKIP_TETRA_CODEC=1 — LST voice encode/decode will be unavailable"
elif [[ -f "$CODEC_SCRIPT" ]]; then
  # Run through bash, no chmod: an executable bit set here is a local change to the checkout,
  # which marks every build "-modified" and makes git refuse the next checkout.
  log "Installing tetra-codec (libtetra-codec) for LST voice"
  if bash "$CODEC_SCRIPT"; then
    WITH_ASTERISK_FEATURE=1
  else
    warn "tetra-codec install failed — continuing without LST voice (signalling only)"
  fi
else
  warn "install-tetra-codec.sh missing — continuing without LST voice"
fi

BUILT_BIN=""
if [[ "${BOST_USE_DEB:-0}" == "1" ]]; then
  warn "BOST_USE_DEB=1 requested but .deb auto-download is not wired yet; building from source"
fi

if [[ "${BOST_SKIP_BUILD:-0}" != "1" ]]; then
  install_rust "$SERVICE_USER"
  CARGO_FEATURES=()
  if [[ "$WITH_ASTERISK_FEATURE" == "1" ]]; then
    CARGO_FEATURES=(--features asterisk)
    log "Building bluestation-bs (release, --features asterisk) — this can take a while on a Pi"
  else
    log "Building bluestation-bs (release) — this can take a while on a Pi"
  fi
  sudo -u "$SERVICE_USER" bash -lc "source \"\$HOME/.cargo/env\" && cd \"$SRC_ROOT\" && cargo build --release -p bluestation-bs ${CARGO_FEATURES[*]:-}"
  BUILT_BIN="$SRC_ROOT/target/release/bluestation-bs"
else
  BUILT_BIN="$SRC_ROOT/target/release/bluestation-bs"
  [[ -x "$BUILT_BIN" ]] || die "BOST_SKIP_BUILD=1 but binary missing at $BUILT_BIN"
fi

install -m 755 "$BUILT_BIN" "$BIN_PATH"
log "Installed $BIN_PATH"

# Initial config (only if missing)
mkdir -p "$CFG_DIR"
if [[ ! -f "$CFG_PATH" ]]; then
  choose_dashboard_ports
  log "Writing initial $CFG_PATH (backend=None, dashboard admin/1234, ota_channel=${OTA_CHANNEL}, ports=${DASH_PORTS_PRESET})"
  if [[ -f "$SRC_ROOT/example_config/config.toml" ]]; then
    cp "$SRC_ROOT/example_config/config.toml" "$CFG_PATH"
    python3 - "$CFG_PATH" "$OTA_CHANNEL" "$DASH_HTTP_PORT" "$DASH_HTTPS_PORT" <<'PY'
import sys, re
path = sys.argv[1]
ota_channel = sys.argv[2] if len(sys.argv) > 2 else "stable"
http_port = sys.argv[3] if len(sys.argv) > 3 else "80"
https_port = sys.argv[4] if len(sys.argv) > 4 else "443"
text = open(path, encoding="utf-8").read()
text = re.sub(r'(?m)^backend\s*=\s*".*"', 'backend = "None"', text, count=1)
if re.search(r'(?m)^\s*service_name\s*=', text):
    text = re.sub(r'(?m)^#?\s*service_name\s*=.*', 'service_name = "bluestation-bs"', text, count=1)
elif re.search(r'(?m)^#\s*service_name\s*=', text):
    text = re.sub(r'(?m)^#\s*service_name\s*=.*', 'service_name = "bluestation-bs"', text, count=1)
else:
    text = 'service_name = "bluestation-bs"\n' + text

# example_config only has a *commented* "# [dashboard]" — that must not count as present.
has_live_dashboard = bool(re.search(r'(?m)^\[dashboard\]\s*$', text))
dash = (
    '\n[dashboard]\n'
    'bind = "0.0.0.0"\n'
    f'port = {http_port}\n'
    f'https_port = {https_port}\n'
    'username = "admin"\n'
    'password = "1234"\n'
    'source_dir = "/opt/bost-flowstation"\n'
    f'ota_channel = "{ota_channel}"\n'
)
if not has_live_dashboard:
    text = text.rstrip() + "\n" + dash
open(path, "w", encoding="utf-8").write(text)
PY
  else
    cat >"$CFG_PATH" <<EOF
# bost-flowstation first-boot config — complete Setup in the web UI
config_version = "0.6"
stack_mode = "Bs"
service_name = "bluestation-bs"

[phy_io]
backend = "None"

[phy_io.soapysdr]
tx_freq = 438025000.0
rx_freq = 433025000.0

[net_info]
mcc = 204
mnc = 1337

[cell_info]
freq_band = 4
main_carrier = 1521
duplex_spacing = 4
freq_offset = 0
reverse_operation = false
location_area = 2
colour_code = 1
local_ssi_ranges = [[0, 90]]
system_wide_services = true
voice_service = true

[dashboard]
bind = "0.0.0.0"
port = ${DASH_HTTP_PORT}
https_port = ${DASH_HTTPS_PORT}
username = "admin"
password = "1234"
source_dir = "/opt/bost-flowstation"
ota_channel = "${OTA_CHANNEL}"
EOF
  fi
  cp "$CFG_PATH" "${CFG_PATH}.fallback"
else
  log "Keeping existing $CFG_PATH"
  # Ensure ota_channel exists / matches install branch when reinstalling with BOST_BRANCH=.
  python3 - "$CFG_PATH" "$OTA_CHANNEL" <<'PY'
import sys, re
path, channel = sys.argv[1], sys.argv[2]
text = open(path, encoding="utf-8").read()
if re.search(r'(?m)^\s*ota_channel\s*=', text):
    text = re.sub(r'(?m)^\s*ota_channel\s*=.*$', f'ota_channel = "{channel}"', text, count=1)
elif re.search(r'(?m)^\[dashboard\]\s*$', text):
    text = re.sub(
        r'(?m)^(\[dashboard\]\s*)$',
        rf'\1\nota_channel = "{channel}"',
        text,
        count=1,
    )
else:
    text = text.rstrip() + f'\n\n[dashboard]\nota_channel = "{channel}"\n'
open(path, "w", encoding="utf-8").write(text)
print(f"ota_channel = {channel}")
PY
fi

# Always ensure a sibling .fallback exists (recovery if primary is hand-edited broken).
# Never overwrite an existing .fallback — it is the operator's known-good anchor.
if [[ ! -f "${CFG_PATH}.fallback" ]]; then
  cp "$CFG_PATH" "${CFG_PATH}.fallback"
  log "Created ${CFG_PATH}.fallback (dashboard + RF-safe boot reserve)"
fi

if [[ ! -f "${CFG_DIR}/setup.json" ]]; then
  cat >"${CFG_DIR}/setup.json" <<'EOF'
{
  "setup_complete": false,
  "skipped": false,
  "version": 1
}
EOF
fi

# Helper + sudoers
install -m 755 "$SRC_ROOT/contrib/install/bost-setup-helper.sh" "$HELPER_DST"
cat >"$SUDOERS_DST" <<EOF
# Allow the FlowStation service user to run only the bost setup helper (no free shell).
${SERVICE_USER} ALL=(root) NOPASSWD: ${HELPER_DST}
Defaults!${HELPER_DST} !requiretty
EOF
chmod 440 "$SUDOERS_DST"
visudo -cf "$SUDOERS_DST" >/dev/null || die "invalid sudoers drop-in"

# NetworkManager Wi-Fi resilience (powersave off). Harmless if NM is absent.
NM_WIFI_SRC="$SRC_ROOT/contrib/install/networkmanager/bost-wifi.conf"
NM_WIFI_DST="/etc/NetworkManager/conf.d/bost-wifi.conf"
if [[ -f "$NM_WIFI_SRC" ]]; then
  if command -v nmcli >/dev/null 2>&1 || [[ -d /etc/NetworkManager ]]; then
    install -d -m 755 /etc/NetworkManager/conf.d
    install -m 644 "$NM_WIFI_SRC" "$NM_WIFI_DST"
    log "Installed $NM_WIFI_DST (wifi.powersave=2)"
    if systemctl is-active --quiet NetworkManager 2>/dev/null; then
      systemctl reload NetworkManager 2>/dev/null \
        || systemctl restart NetworkManager 2>/dev/null \
        || warn "could not reload NetworkManager — reboot or: systemctl reload NetworkManager"
    fi
  else
    warn "NetworkManager not detected — skipped Wi-Fi conf drop-in"
  fi
fi

# systemd unit
UNIT_DST="/etc/systemd/system/${UNIT_NAME}"
SERVICE_HOME="$(getent passwd "$SERVICE_USER" | cut -d: -f6)"
SERVICE_HOME="${SERVICE_HOME:-/home/${SERVICE_USER}}"
CARGO_BIN="${SERVICE_HOME}/.cargo/bin/cargo"
CARGO_ENV_LINE=""
if [[ -x "$CARGO_BIN" ]]; then
  CARGO_ENV_LINE="Environment=CARGO=${CARGO_BIN}"
fi
cat >"$UNIT_DST" <<EOF
[Unit]
Description=bost-flowstation TETRA base station (bluestation-bs)
Documentation=https://github.com/Aitorrio/bost-flowstation
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=300
StartLimitBurst=10

[Service]
Type=simple
User=root
Environment=BOST_SETUP_HELPER=${HELPER_DST}
Environment=BOST_SERVICE_UNIT=${UNIT_NAME}
Environment=BOST_SERVICE_USER=${SERVICE_USER}
Environment=BOST_SRC=${SRC_ROOT}
${CARGO_ENV_LINE}
ExecStart=${BIN_PATH} ${CFG_PATH}
# Real-time scheduling helps the PHY loop when RF is enabled.
CPUSchedulingPolicy=fifo
CPUSchedulingPriority=73
KillSignal=SIGINT
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now "$UNIT_NAME"

# Primary LAN IP hint
IP="$(hostname -I 2>/dev/null | awk '{print $1}')"
IP="${IP:-<pi-ip>}"

echo
echo "────────────────────────────────────────────────────────"
echo " Bost FlowStation installed"
if [[ "${DASH_HTTPS_PORT:-443}" == "443" ]]; then
  echo " Dashboard:  https://${IP}/  (HTTP :80 redirects to HTTPS)"
else
  echo " Dashboard:  https://${IP}:${DASH_HTTPS_PORT}/  (HTTPS only; no HTTP :80)"
fi
echo " Login:      admin / 1234"
echo " Config:     ${CFG_PATH}"
echo " Setup:      open the Setup tab / first-run wizard"
echo " RF starts disabled (backend=None) until you finish Setup."
echo " Repo:       https://github.com/Aitorrio/bost-flowstation (branch ${BRANCH}, OTA ${OTA_CHANNEL})"
if [[ "$WITH_ASTERISK_FEATURE" == "1" ]]; then
  echo " Voice:      libtetra-codec installed (LST Dispatch ACELP linked)"
else
  echo " Voice:      signalling only — install codec later via OTA or:"
  echo "             sudo bash ${SRC_ROOT}/contrib/install/install-tetra-codec.sh"
fi
echo "────────────────────────────────────────────────────────"
