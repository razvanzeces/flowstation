# FlowStation

> **TETRA base station software for amateur radio operators and researchers.**
> Built in Rust. Runs on a Raspberry Pi with a LimeSDR. Works with real TETRA radios.

FlowStation is a fork of [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) (MidnightBlueLabs), developed and maintained by **Razvan Zeces / YO6RZV**.

Tested hardware: **LimeSDR Mini 2.0** · **Motorola MXP600** · **Motorola MTM800E** · **Motorola MTM5400**

---

## What it does

FlowStation implements a fully functional TETRA base station (BS) in software. You plug in a supported SDR, point it at your TETRA radios, and get:

- Group calls, individual (P2P) calls, half-duplex PTT — all working
- SDS messaging (text messages between radios)
- Network interconnect via [Brew / TetraPack](https://wiki.tetrapack.online/books/tetra/page/brew) — connects your local cell to BrandMeister or TetraPack
- UTC time broadcast so radios sync their clocks automatically
- A web dashboard at `http://<bts-ip>:8080` for monitoring and remote management

---

## Feature overview

| Feature | Status |
|---|---|
| Group calls (local) | ✅ |
| Group calls via Brew (BrandMeister / TetraPack) | ✅ |
| Full-duplex P2P calls (local + Brew) | ✅ |
| Half-duplex P2P calls (simplex PTT) | ✅ |
| SDS forwarding (local + Brew) | ✅ |
| UTC time broadcast (D-NWRK-BROADCAST) | ✅ |
| T351 periodic re-registration | ✅ |
| Home Mode Display (PID 220 callsign) | ✅ |
| Supplemental SDS broadcast (custom PID) | ✅ |
| ISSI whitelist (access control) | ✅ |
| Local SSI ranges (local-only traffic) | ✅ |
| Remote control via U-STATUS from radio | ✅ |
| Neighbor cell broadcast | ✅ |
| Web dashboard | ✅ |
| OTA update button | ✅ |
| HTTP Basic Auth on dashboard | ✅ |
| Fallback config on bad edit | ✅ |
| Live SDS broadcast queue | ✅ |
| Edit inactive config profiles in dashboard | ✅ |
| System / RF hardware tab | ✅ |
| Coordinated handover | 🔜 |
| Emergency calls | 🔜 |
| Authentication (TEA) | 🔜 |
| AIE encryption | 🔜 |
| Multi-carrier (2× SDR) | 🔜 |

---

## Installation

### Requirements

- **Rust** — latest stable (`rustup update stable`)
- **SoapySDR** with drivers for your SDR
- A supported SDR — LimeSDR Mini 2.0 is the reference hardware

### From git

```bash
git clone https://github.com/razvanzeces/flowstation.git
cd flowstation
cp example_config/config.toml ./config.toml
# Edit config.toml — at minimum set tx_freq, rx_freq, mcc, mnc
cargo build --release
./target/release/bluestation-bs config.toml
```

### As a systemd service

A sample unit file is in `contrib/systemd/`. Copy and adapt it:

```bash
cp contrib/systemd/bluestation-bs.service /etc/systemd/system/tetra.service
# Edit paths and user
systemctl daemon-reload
systemctl enable --now tetra
```

The service name (`tetra`) must match the `service_name` used in any restart/shutdown commands.

---

## Configuration

The full config is documented in `example_config/config.toml`. Key sections:

### Mandatory

```toml
[phy_io.soapysdr]
tx_freq = 438025000   # DL frequency in Hz
rx_freq = 433025000   # UL frequency in Hz

[net_info]
mcc = 204             # Mobile Country Code
mnc = 1337            # Mobile Network Code

[cell_info]
freq_band = 4         # 400 MHz band
main_carrier = 1521
duplex_spacing = 4
location_area = 2
colour_code = 1
```

### Timing (FlowStation-specific)

| Parameter | Default | Description |
|---|---|---|
| `hangtime_secs` | `5` | Hold group call circuit after floor release (0–300s) |
| `call_timeout_secs` | `120` | Max call duration before forced D-RELEASE (0 = unlimited) |
| `ul_inactivity_secs` | `3` | UL silence before forced TX-CEASED (1–30s) |
| `periodic_registration_secs` | `3600` | T351 interval; `0` = disabled |

### Dashboard

```toml
[dashboard]
port = 8080

# Optional: HTTP Basic Auth
username = "admin"
password = "changeme"

# Optional: explicit git source path for OTA updates
# source_dir = "/opt/flowstation"
```

### Fallback config

If FlowStation fails to parse `config.toml` at startup (e.g. after a bad edit in the dashboard), it automatically tries `config.toml.fallback`. Create it once from a known-good config:

```bash
cp config.toml config.toml.fallback
```

When running on fallback, the dashboard shows a persistent red warning banner with the parse error, so you can fix the primary config remotely without losing access.

### Home Mode Display (callsign on radio screen)

```toml
[cell_info.home_mode_display]
text = "YO6RZV"              # Shown on radio home screen
interval_multiframes = 96    # ≈ 96 seconds
protocol_id = 220
text_coding_scheme = "LATIN"
```

### Access control

```toml
[security]
issi_whitelist = [2260571, 2260572]   # Only these ISSIs can register
```

### Remote control from radio (U-STATUS)

```toml
[cell_info.sds_command_control]
authorized_issis = [2260570, 2260571]

[[cell_info.sds_command_control.commands]]
status_code = 32001
action = "restart"        # restart / shutdown / kick_all

[[cell_info.sds_command_control.commands]]
status_code = 32003
action = "kick_all"
```

### Brew (TetraPack / BrandMeister interconnect)

```toml
[brew]
host = "core.tetraflow.ro"
port = 9000
tls = true
username = 123456700
password = "hotspot_password"
```

---

## Web dashboard

Available at `http://<bts-ip>:8080` when `[dashboard]` is configured.

**Radios tab** — live table of registered terminals with ISSI, groups, RSSI signal bar, energy saving mode, last seen time. Kick button forces immediate re-registration. SDS button sends a text message. Timeslot visualizer shows TS2–TS4 state in real time — idle (grey), call allocated (amber), voice active (red flash with animated waveform).

**Calls tab** — active calls with caller, destination, duration, simplex/duplex.

**Last Heard** — rolling history of call starts and SDS activity.

**Log tab** — live log with level filter and autoscroll.

**Config tab** — edit the active `config.toml` directly. Save writes to disk; restart applies changes. Backup and restore buttons.

**System tab:**
- BTS / Brew connection status
- System uptime, hostname
- CPU model, core count, load bar, RAM usage bar
- CPU temperature (where available)
- RF hardware info (SoapySDR probe output)
- Auto-refresh checkbox (5s interval)
- Config profiles — activate, edit inactive profiles directly in a modal editor
- Live SDS broadcast queue — broadcast a text message to all radios on the cell, repeating at the HMD interval until deleted or repeat count exhausted
- OTA update — pulls latest from `main` branch, rebuilds, restarts

---

## Key fixes vs upstream

**ExpiryOfTimer crash loop** — `release_group_call` now sends `NetworkCallEnd` to Brew when a network-initiated group call expires. Without this, Brew kept the call alive and re-issued `NetworkCallStart` with new speakers, generating thousands of `ExpiryOfTimer` releases per minute and crashing the stack.

**Simplex P2P (half-duplex PTT)** — `transmission_request_permission` correctly set to `false` in `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED`, and `D-TX-GRANTED`. On `U-TX-CEASED`, BS sends `D-TX-CEASED` to the speaker and `D-TX-GRANTED(Granted)` to the peer — terminals that receive `GrantedToOtherUser` in `D-CONNECT` need an explicit `D-TX-GRANTED` to unlock PTT, `D-TX-CEASED` alone is not enough.

**Sepura post-PTT RoamingLocationUpdating** — Sepura terminals send `RoamingLocationUpdating` after every PTT release, not just on power cycle. Without the heuristic (< 60s since last registration → treat as soft re-attach), CMCE briefly loses track of the terminal and the next PTT is denied. Fixed with timing-based soft re-attach detection.

**BCD external subscriber number** — decoder was shifting from nibble count instead of from bit 64, producing incorrect ISSI values in certain call scenarios.

**UL audio routing to Brew** — `TmdCircuitDataInd` was not routed to Brew in `cmce_bs.rs`, causing one-way audio on Brew-interconnected calls.

**SDS ACK for ISSI 9999** — SDS ACK for the local BS control ISSI was being forwarded to Brew, generating spurious traffic. Now absorbed locally.

**Chan_alloc in DConnect for echo service 999** — echo service calls were being allocated without a traffic channel, causing audio to fail.

---

## Branches

| Branch | Purpose |
|---|---|
| `main` | Stable, tested releases |
| `beta` | Work in progress, new features |

---

## Credits

- **Harald Welte** and the **osmocom** team for the foundational osmocom-tetra work
- **Tatu Peltola** for rust-soapysdr timestamping and the native Rust Viterbi encoder/decoder used in LMAC
- **MidnightBlueLabs** for tetra-bluestation, the base this project builds on
- **Stichting NLnet** for partially funding this work through the [RETETRA3 grant](https://nlnet.nl/project/RETETRA3/)
- The FlowStation user community — ON6RF, EA7KEN, BU2GQ, DK5RTA, DO5MF, ES4TIX, DK5RTA and others — for testing, bug reports, and feature requests that shaped this release

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
