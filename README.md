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

- **Harald Welte** and the **osmocom** team for foundational osmocom-tetra work
- **Tatu Peltola** for rust-soapysdr timestamping and the native Rust Viterbi encoder/decoder used in LMAC
- **MidnightBlueLabs** for [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation), the base this project builds on
- **Stichting NLnet** for partially funding this work through the [RETETRA3 grant](https://nlnet.nl/project/RETETRA3/)
- The FlowStation community — ON6RF, EA7KEN, BU2GQ, DK5RTA, DO5MF, ES4TIX and others — for testing, bug reports, and feature requests

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
