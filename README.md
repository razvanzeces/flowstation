<div align="center">

<img src="contrib/logo/flowstation_logo.png" alt="FlowStation" width="360"/>

### Software-defined TETRA base station — built in Rust, runs on a Raspberry Pi.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Website](https://img.shields.io/badge/website-flowstation.dev-informational)](https://flowstation.dev)
[![Telegram](https://img.shields.io/badge/community-Telegram-2CA5E0?logo=telegram)](https://t.me/+fktnT-th7dcxYWNk)

**[Website](https://flowstation.dev) · [Install Guide](https://install.flowstation.dev) · [Bug Tracker](https://hub.flowstation.dev) · [Live Stats](https://stats.flowstation.dev) · [Telegram](https://t.me/+fktnT-th7dcxYWNk)**

</div>

---

## What is FlowStation?

FlowStation is a fully functional **TETRA base station in software**. Plug in a LimeSDR, point it at your TETRA radios, and you have a working private TETRA cell — group calls, individual calls, SDS messaging, Brew/BrandMeister interconnect, and a live web dashboard. No proprietary infrastructure required.

Built in Rust on top of [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation), maintained by **Razvan Zeces / YO6RZV**.

**Tested hardware:** LimeSDR Mini 2.0 · Motorola MXP600 · Motorola MTM800E · Motorola MTM5400

---

## Features

### Voice & Calls
| Feature | Status |
|---|---|
| Group calls (local) | ✅ |
| Group calls via Brew (BrandMeister / TetraPack) | ✅ |
| Full-duplex individual (P2P) calls — local + Brew | ✅ |
| Half-duplex P2P calls (simplex PTT) | ✅ |
| Call hangtime (configurable hold after floor release) | ✅ |
| Max call duration with forced D-RELEASE | ✅ |
| UL inactivity detection (forced TX-CEASED) | ✅ |
| Echo service (local loopback, ISSI 999) | ✅ |
| Coordinated handover | 🔜 |
| Emergency call pre-emption (priority calls pre-empt a lower-priority call when the cell is full) | ✅ |

### Messaging
| Feature | Status |
|---|---|
| SDS forwarding — local + Brew | ✅ |
| Live SDS broadcast queue (send to all radios, with repeat) | ✅ |
| Home Mode Display (PID 220 callsign on radio screen) | ✅ |
| Supplemental SDS broadcast (custom PID) | ✅ |
| Emergency status alarm (U-STATUS) — persistent dashboard banner + Telegram alert, LOCAL-only | ✅ |

### Network & Interconnect
| Feature | Status |
|---|---|
| Brew / TetraPack / BrandMeister interconnect | ✅ |
| UTC time broadcast (D-NWRK-BROADCAST) | ✅ |
| Neighbor cell broadcast | ✅ |
| T351 periodic re-registration | ✅ |
| Multi-carrier (2× SDR) | 🔜 |

### Security & Access Control
| Feature | Status |
|---|---|
| ISSI whitelist (only registered ISSIs can use the cell) | ✅ |
| Local SSI ranges (local-only traffic isolation) | ✅ |
| Authentication (TEA) | 🔜 |
| AIE encryption | 🔜 |

### Management & Dashboard
| Feature | Status |
|---|---|
| Web dashboard (Radios, Calls, Last Heard, Log, Config, System) | ✅ |
| HTTP Basic Auth on dashboard | ✅ |
| Live timeslot visualizer (TS2–TS4 state, call/voice indicator) | ✅ |
| Kick terminal / send SDS from dashboard | ✅ |
| Dynamic Group Number Assignment (DGNA) — assign/remove a talkgroup on a radio over the air from the dashboard | ✅ |
| Config editor with save, backup, restore | ✅ |
| Multiple config profiles — activate and edit inactive profiles | ✅ |
| Fallback config on bad edit (with dashboard error banner) | ✅ |
| Remote control via U-STATUS from radio (restart, shutdown, kick_all) | ✅ |
| OTA update (pull latest, rebuild, restart — one button) | ✅ |
| System tab: uptime, CPU, RAM, temperature, RF hardware info | ✅ |

---

## Installation

Full step-by-step installation guide (Raspberry Pi + LimeSDR): **[install.flowstation.dev](https://install.flowstation.dev)**

### Quick start (from source)

```bash
git clone https://github.com/razvanzeces/flowstation.git
cd flowstation
cp example_config/config.toml ./config.toml
# Edit config.toml — set tx_freq, rx_freq, mcc, mnc at minimum
cargo build --release
./target/release/bluestation-bs config.toml
```

### As a systemd service

```bash
cp contrib/systemd/bluestation-bs.service /etc/systemd/system/tetra.service
# Edit paths and user in the unit file
systemctl daemon-reload
systemctl enable --now tetra
```

---

## Configuration

The fully annotated reference config is at [`example_config/config.toml`](example_config/config.toml). Below are the essentials.

### Mandatory

```toml
[phy_io.soapysdr]
tx_freq = 438025000   # Downlink frequency in Hz
rx_freq = 433025000   # Uplink frequency in Hz

[net_info]
mcc = 204             # Mobile Country Code
mnc = 1337            # Mobile Network Code

[cell_info]
freq_band = 4         # 4 = 400 MHz band
main_carrier = 1521
duplex_spacing = 4
location_area = 2
colour_code = 1
```

### Timing

| Parameter | Default | Description |
|---|---|---|
| `hangtime_secs` | `5` | Hold group call circuit after floor release (0–300s) |
| `call_timeout_secs` | `120` | Max call duration before forced D-RELEASE (0 = unlimited) |
| `ul_inactivity_secs` | `3` | UL silence before forced TX-CEASED (1–30s) |
| `periodic_registration_secs` | `3600` | T351 interval; `0` = disabled |

### Brew interconnect (BrandMeister / TetraPack)

```toml
[brew]
host = "core.tetraflow.ro"
port = 9000
tls = true
username = 123456700
password = "your_password"
```

### Access control

```toml
[security]
issi_whitelist = [2260571, 2260572]   # Only these ISSIs can register
```

### Home Mode Display (callsign on radio screen)

```toml
[cell_info.home_mode_display]
text = "YO6RZV"
interval_multiframes = 96
protocol_id = 220
text_coding_scheme = "LATIN"
```

### Remote control from radio (U-STATUS)

```toml
[cell_info.sds_command_control]
authorized_issis = [2260570, 2260571]

[[cell_info.sds_command_control.commands]]
status_code = 32001
action = "restart"

[[cell_info.sds_command_control.commands]]
status_code = 32003
action = "kick_all"
```

### Fallback config

If FlowStation fails to parse `config.toml` at startup (e.g. after a bad dashboard edit), it falls back to `config.toml.fallback` automatically. Create it once:

```bash
cp config.toml config.toml.fallback
```

The dashboard shows a persistent red warning banner with the parse error so you can fix the config remotely without losing access to the cell.

---

## Integrations

FlowStation can bridge TETRA to external paging and telephony networks and push
alerts out to desk phones, dashboards, and Telegram. DAPNET, Snom display
notifications, GeoAlarm, and the TPG2200 trigger are all part of the **default
build** — just fill in their config sections. Asterisk SIP/RTP telephony is
**feature-gated** (see below).

> **Asterisk is not in the default build.** To use the SIP/RTP bridge the device
> binary must be built with `cargo build --release --features asterisk`, and the
> native [`tetra-codec`](https://github.com/outerplane/tetra-codec) (outerplane)
> ACELP library must be installed so FlowStation can convert between TETRA ACELP
> and PCM audio. The default `cargo build --release` does **not** include Asterisk;
> DAPNET, Snom notify, GeoAlarm, the TPG2200 trigger, and the dashboard all work
> without it.

### TETRA ACELP codec (Asterisk only)

The Asterisk audio bridge needs a TETRA ACELP codec implementation. One tested
implementation is `outerplane/tetra-codec`:

```bash
git clone https://github.com/outerplane/tetra-codec
cd tetra-codec
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build --parallel
sudo cmake --install build
sudo ldconfig
```

After installation, confirm both TETRA ACELP -> PCM decoding and PCM -> TETRA
ACELP encoding work before enabling the bridge (follow the codec project's
README for the exact verification commands).

### Asterisk packages and modules

Install Asterisk when using the SIP bridge or Snom display notifications:

```bash
sudo apt install -y asterisk
sudo systemctl enable --now asterisk
```

For Snom XML display notifications, Asterisk must be able to load
`res_pjsip_notify.so`. Some distributions refuse to load it if
`pjsip_notify.conf` is missing, so create the file:

```bash
sudo touch /etc/asterisk/pjsip_notify.conf
sudo asterisk -rx "module load res_pjsip_notify.so"
sudo asterisk -rx "module show like pjsip_notify"
```

Enable AMI in `/etc/asterisk/manager.conf` for FlowStation's Snom notify worker:

```ini
[general]
enabled = yes
webenabled = no
bindaddr = 127.0.0.1
port = 5038

[flowstation]
secret = change-this-password
read = system,call,log,verbose,command,agent,user
write = system,call,command,agent,user,originate
```

Reload Asterisk after edits:

```bash
sudo asterisk -rx "manager reload"
sudo asterisk -rx "module reload res_pjsip.so"
sudo asterisk -rx "dialplan reload"
```

If you run FlowStation behind a firewall, open the ports that match your config:

| Component | Default port(s) | Direction |
|---|---:|---|
| FlowStation SIP | UDP/TCP 5062 | inbound from Asterisk |
| FlowStation RTP | UDP 30000-30100 | inbound/outbound to Asterisk |
| Asterisk SIP | UDP/TCP 5060 | outbound/inbound to PBX |
| DAPNET RWTH core | TCP 43434 | outbound |

### Asterisk SIP/RTP bridge

FlowStation can register as a PJSIP endpoint and bridge calls between TETRA
terminals and Asterisk phones. Brew remains available in parallel; only configured
service numbers are routed to Asterisk.

This bridge requires a build with `--features asterisk` and the native
`tetra-codec` library (see the note at the top of this section); it is not part of
the default build.

```toml
[asterisk]
enabled = true
outbound_prefix = "91"          # TETRA -> SIP: 91385 calls SIP user 385
strip_outbound_prefix = true
inbound_prefix = "T"            # SIP -> TETRA: Dial PJSIP/T2632585@flowstation
register = true
codec = "PCMU"                  # currently the only supported SIP codec
service_numbers = ["385", "600", "601"]
rtp_port_min = 30000
rtp_port_max = 30100
bind_addr = "0.0.0.0"
bind_port = 5062
remote_host = "127.0.0.1"
remote_port = 5060
contact_host = "127.0.0.1"
from_domain = "127.0.0.1"
local_user = "flowstation"
auth_user = "flowstation"
password = "change-me"
realm = "asterisk"
```

Minimal Asterisk dialplan shape:

```ini
[intern]
; Snom/internal phone -> TETRA
_91X. => 1,Dial(PJSIP/T${EXTEN:2}@flowstation,60)
 same => n,Hangup()

[tetra]
; TETRA -> Snom/internal phone
385 => 1,Dial(PJSIP/snom385,30)
 same => n,Hangup()
```

Minimal PJSIP endpoint shape for FlowStation:

```ini
; /etc/asterisk/pjsip.conf
[flowstation]
type=endpoint
transport=transport-udp
context=tetra
disallow=all
allow=ulaw
aors=flowstation
auth=flowstation

[flowstation]
type=auth
auth_type=userpass
username=flowstation
password=change-me

[flowstation]
type=aor
max_contacts=1
remove_existing=yes
qualify_frequency=30
```

`service_numbers` is deliberately an allowlist. If a TETRA user dials `91385`,
FlowStation strips `91`, checks that `385` is listed, then calls SIP user `385`.

### DAPNET integration

DAPNET receive uses the RWTH core feed. Messages can be routed independently to:

- normal TETRA SDS
- Motorola TPG2200 Call-Out / Type-4 SDS
- Telegram

RIC routing is important: a DAPNET RIC may be a single-user RIC or a group RIC.
Use the route maps and allowlists to decide which RICs are forwarded and where.

```toml
[dapnet]
enabled = true
api_url = "https://hampager.de/api/calls"
username = "YOURCALL"
password = "your-hampager-password"
poll_interval_secs = 30

forward_sds = true
forward_callout = false
forward_telegram = true

sds_source_issi = 9999
sds_dest_issi = 0
sds_dest_is_group = false
ric_issi_routes = { "0632585" = 2632585 }
ric_gssi_routes = { "0004520" = 80 }
sds_allowed_rics = ["0632585", "0004520"]
callout_allowed_rics = []
telegram_allowed_rics = []

callout_source_issi = 9999
callout_dest_issi = 0
callout_incident_base = 2
callout_text_prefix = "DAPNET"

telegram_prefix = "DAPNET"

rwth_core_enabled = true
rwth_core_host = "dapnet.afu.rwth-aachen.de"
rwth_core_port = 43434
rwth_core_device = "FlowStation"
rwth_core_version = "1.0"
rwth_core_callsign = "YOURCALL"
rwth_core_authkey = "your-rwth-core-authkey"
rwth_messages_limit = 100
```

Keep `password` and `rwth_core_authkey` private and out of commits.

### Motorola TPG2200 ActionURL trigger

FlowStation can expose a token-protected HTTP endpoint so a Snom function key
can trigger a Motorola TPG2200 Call-Out. Every accepted request increments the
incident number in memory and wraps after 256.

```toml
[tpg2200_action]
enabled = true
token = "long-random-token"
source_issi = 9999
dest_issi = 2632585
incident_base = 1
default_text = "ALARM"
max_text_chars = 80
```

Snom ActionURL examples:

```text
http://<flowstation>:8080/api/action/tpg2200?token=<token>
http://<flowstation>:8080/api/action/tpg2200?token=<token>&text=ALARM
```

### Snom XML display notifications

FlowStation can show SDS, DAPNET, and Telegram messages on Snom phones
as `SnomIPPhoneText`. FlowStation does not send direct SIP NOTIFY to the phone;
it asks Asterisk over AMI to execute `PJSIPNotify`, so Asterisk remains the SIP
sender.

```toml
[snom_notify]
enabled = true
ami_host = "127.0.0.1"
ami_port = 5038
ami_username = "flowstation"
ami_password = "change-me"
endpoints = ["385"]
notify_sds = true
notify_dapnet = true
notify_telegram = true
sds_directions = ["rx", "net", "tx"]
dapnet_allowed_rics = []       # empty = all RICs
sds_allowed_issis = []         # empty = all source/destination ISSIs
title_prefix = "FlowStation"
notify_event = "xml"
content_type = "application/snomxml"
subscription_state = "active;expires=30000"
max_text_chars = 240
connect_timeout_secs = 3
```

Snom phone settings must allow XML/minibrowser notifications. In Asterisk, make
sure AMI is enabled and `res_pjsip_notify.so` loads. This worker only needs
Asterisk AMI; it does not require the feature-gated SIP/RTP bridge.

### GeoAlarm

GeoAlarm watches decoded TETRA LIP SDS positions. When an allowed device enters
the configured radius around FlowStation, it can trigger TPG2200 Call-Out,
normal SDS, Snom/SIP display notification, and Telegram forwarding. Blacklists
always win; empty whitelists mean "all".

The `*_meshcom*` toggles below are accepted by the parser but inert in this
build: FlowStation does not ship a MeshCom source, so positions come from TETRA
LIP only. Leave the MeshCom options at their defaults.

```toml
[geoalarm]
enabled = false
flowstation_lat = 50.775346
flowstation_lon = 6.083887
radius_m = 500
cooldown_secs = 300

trigger_tetra = true
trigger_meshcom = true

forward_tpg2200 = false
forward_sds = false
forward_sip = false
forward_telegram = false

tetra_issi_whitelist = []       # empty = all TETRA ISSIs
tetra_issi_blacklist = []
meshcom_source_whitelist = []   # inert: no MeshCom source in this build
meshcom_source_blacklist = []

sds_source_issi = 9999
sds_dest_issi = 0
sds_dest_is_group = false

tpg2200_source_issi = 9999
tpg2200_dest_issi = 0
tpg2200_incident_base = 1
tpg2200_text_prefix = "GeoAlarm"
tpg2200_max_text_chars = 80

sip_title_prefix = "GeoAlarm"
telegram_prefix = "GeoAlarm"
```

---

## Web Dashboard

Available at `http://<bts-ip>:8080` when `[dashboard]` is configured.

**Radios** — live table of registered terminals: ISSI, groups, RSSI signal bar, energy saving mode, last seen. Kick and SDS buttons per radio. Timeslot visualizer shows TS2–TS4 state in real time (idle / call allocated / voice active with animated waveform).

**Calls** — active calls: caller, destination, duration, simplex/duplex flag.

**Last Heard** — rolling history of call starts and SDS activity.

**Log** — live log stream with level filter and autoscroll.

**Config** — edit `config.toml` in-browser. Save, backup, restore. Edit inactive config profiles in a modal without switching them live.

**System** — BTS and Brew connection status · uptime · hostname · CPU model, cores, load bar · RAM usage · CPU temperature · RF hardware info (SoapySDR probe) · SDS broadcast queue · OTA update button.

---

## Key fixes vs upstream

**ExpiryOfTimer crash loop** — `release_group_call` now sends `NetworkCallEnd` to Brew when a network-initiated group call expires. Without this, Brew kept the call alive and re-issued `NetworkCallStart` with new speakers, generating thousands of `ExpiryOfTimer` releases per minute and crashing the stack.

**Simplex P2P (half-duplex PTT)** — `transmission_request_permission` correctly set to `false` in `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED`, and `D-TX-GRANTED`. On `U-TX-CEASED`, BS sends `D-TX-CEASED` to the speaker and `D-TX-GRANTED(Granted)` to the peer — terminals receiving `GrantedToOtherUser` in `D-CONNECT` need an explicit `D-TX-GRANTED` to unlock PTT; `D-TX-CEASED` alone is not enough.

**Sepura post-PTT RoamingLocationUpdating** — Sepura terminals send `RoamingLocationUpdating` after every PTT release. Without timing-based soft re-attach detection (< 60s since last registration → treat as re-attach), CMCE loses track of the terminal and the next PTT is denied.

**BCD external subscriber number** — decoder was shifting from nibble count instead of from bit 64, producing incorrect ISSI values in certain call scenarios.

**UL audio routing to Brew** — `TmdCircuitDataInd` was not routed to Brew in `cmce_bs.rs`, causing one-way audio on Brew-interconnected calls.

**SDS ACK for ISSI 9999** — SDS ACK for the local BS control ISSI was being forwarded to Brew, generating spurious traffic. Now absorbed locally.

**Chan_alloc in DConnect for echo service 999** — echo service calls were allocated without a traffic channel, causing audio to fail.

---

## Branches

| Branch | Purpose |
|---|---|
| `main` | Stable, tested releases |
| `alpha` | Active development — new features, may be rough |

---

## Community & Support

- **Website:** [flowstation.dev](https://flowstation.dev)
- **Installation guide:** [install.flowstation.dev](https://install.flowstation.dev)
- **Bug reports & feature requests:** [hub.flowstation.dev](https://hub.flowstation.dev)
- **Live network stats:** [stats.flowstation.dev](https://stats.flowstation.dev)
- **Telegram group:** [t.me/+fktnT-th7dcxYWNk](https://t.me/+fktnT-th7dcxYWNk)

---

## Credits

- **Mihajlo YU4MSH** ([misadeks](https://github.com/misadeks)) for contributions to full-duplex (P2P) calls and the Home Mode Display feature + all the continued support.
- **Torben DJ2TH** ([Torben-DJ2TH](https://github.com/Torben-DJ2TH)) for the external integrations: DAPNET paging, Asterisk SIP/PSTN telephony, Snom desk-phone notifications, and GeoAlarm geofencing.
- **Joaquin EA5GVK** ([ea5gvk](https://github.com/ea5gvk)) for fixing dashboard-composed SDS routing — SDS to non-local destinations now go over the Brew link instead of being lost on RF.
- **Harald Welte** and the **osmocom** team for foundational osmocom-tetra work
- **Tatu Peltola** for rust-soapysdr timestamping and the native Rust Viterbi encoder/decoder used in LMAC
- **MidnightBlueLabs** for [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation), the base this project builds on
- **Stichting NLnet** for partially funding this work through the [RETETRA3 grant](https://nlnet.nl/project/RETETRA3/)
- The FlowStation community — ON6RF, EA7KEN, BU2GQ, DK5RTA, DO5MF, ES4TIX and others — for testing, bug reports, and feature requests

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
