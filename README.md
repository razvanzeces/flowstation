```
░█▀▀░█░░░█▀█░█░░░█░█░█▀▀░▀█▀░█▀█░▀█▀░▀█▀░█▀█░█▀█
░█▀▀░█░░░█░█░█░█░█░█░▀▀█░░█░░█▀█░░█░░░█░░█░█░█░█
░▀░░░▀▀▀░▀▀▀░▀▀▀▀░▀▀▀░▀▀▀░░▀░░▀░▀░░▀░░▀▀▀░▀▀▀░▀░▀
```

**FlowStation** is a fork of [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) by MidnightBlueLabs — an open-source TETRA base station implementation written in Rust. FlowStation adds stability fixes, critical bug corrections, and extended functionality on top of the upstream project.

> **Contact:** Telegram [@razvanzeces](https://t.me/razvanzeces)

---

## Branches

| Branch | Purpose |
|--------|---------|
| `main` | Stable, tested releases |
| `beta` | Work in progress, new features |

---

## Current Status

### Group Calls (Talkgroup)
- ✅ Local group calls between radios connected to the BS
- ✅ Group calls via Brew (BrandMeister / TetraPack) — full support both locally and over network
- ✅ Real-time speaker change on active group calls
- ✅ Configurable hangtime after floor release
- ✅ Late entry (joining active calls)

### Individual Calls (P2P)
- ✅ **Full duplex** individual calls — fully working locally and via Brew
- ✅ Individual calls via Brew (circuit-switched P2P over network)
- ✅ **Half-duplex** (simplex PTT P2P) individual calls — implemented and tested on real hardware

### Connectivity & Network
- ✅ BrandMeister / TetraPack network connectivity via Brew protocol
- ✅ Talkgroup affiliation and automatic routing
- ✅ SDS forwarding between local clients and Brew
- ✅ UTC time broadcast via D-NWRK-BROADCAST (radio clock synchronization)

### Energy Economy (EE)
- ✅ Eg1/Eg2/Eg3 energy saving mode support
- ✅ SCCH information for terminals with scan list active (fixes PTT blocked on MXP600 in TMO mode)
- ✅ P2P DSetup retry for sleeping terminals — BS retransmits DSetup every 10 seconds while call is pending, so a sleeping MS receives it at its next monitoring window

### Neighbor Cell Broadcast (NCB)
- ✅ BS broadcasts neighbor cell information in D-NWRK-BROADCAST
- ✅ Terminals use this list for automatic cell reselection between multiple BS units
- ✅ Up to 7 neighbor cells configurable in `config.toml`
- ✅ Correct serialization for any number of neighbors (O-bit per ETSI EN 300 392-2 §18.5.17)

### Security
- ✅ ISSI whitelist — only listed terminals can register on the BS

### Logging
- ✅ Local system time timestamp on every log line (HH:MM:SS.mmm)

---

## Improvements over Upstream

### Critical crash fix — bs_sched assert panic
Replaced `assert! + swap` with `extend/prepend` on `dltx_next_slot_queue`. The assert would panic when two back-to-back P2P calls each deferred a `chan_alloc` PDU within the same TDMA tick, crashing the entire BS.

### Fix — terminal loses signal after call ends
`D-RELEASE` on active calls is now sent on MCCH without `chan_alloc`. Previously it was sent via FACCH stealing with an active `chan_alloc: Replace`, causing the terminal to attempt channel allocation while processing the release — leaving it stuck on the assigned timeslot with no signal.

### Fix — ExpiryOfTimer loop stability (upstream)
`release_group_call` now notifies Brew with `NetworkCallEnd` on group call expiry. Without this fix, Brew kept the call active and continuously sent `NetworkCallStart` with new speakers, generating thousands of `ExpiryOfTimer` events and general stack instability.

### Half-duplex P2P fixes
- `transmission_request_permission` correctly set to `false` (= 0 = permitted) in `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED` and `D-TX-GRANTED` — fixes "Not allowed to transmit" on Motorola/Sepura radios.
- On receiving `U-TX-CEASED`, BS sends `D-TX-CEASED` to the speaker and explicit `D-TX-GRANTED(Granted)` to the peer. Radios that receive `GrantedToOtherUser` in `D-CONNECT` require an explicit `D-TX-GRANTED` to activate the PTT button — `D-TX-CEASED` alone is not sufficient.

### Log noise reduction
Frequent false warnings (`setting expected ack for ts1`, `brew_uuid changed during speaker change`, `UFacility`) demoted to `trace`/`debug` level, as they represent normal behavior rather than actual errors.

---

## Hardware

FlowStation uses [SoapySDR](https://github.com/pothosware/SoapySDR) and supports any SDR compatible with it, including:

- **LimeSDR** (Mini, USB, X3) — tested
- **Analog Devices ADALM-Pluto (PlutoSDR / Pluto+)** — tested
- **HackRF One**
- **USRP** (Ettus Research)
- **Airspy**
- **RTL-SDR** (receive-only, not suitable for BS use)
- Any other SoapySDR-compatible device

Check your device with `SoapySDRUtil --probe` and set the appropriate `device`, `rx_antenna`, `tx_antenna`, and gain values in `config.toml`.

---

## Installation

Download the archive from [Releases](../../releases), extract and follow the steps:

```bash
tar -xzf flowstation-v*.tar.gz
cd tetra-bluestation
cp example_config/config.toml ./config.toml
# edit config.toml for your parameters
cargo build --release
```

> The extracted folder is `tetra-bluestation/` for compatibility with existing upstream documentation and scripts.

---

## Build from Source

Requirements: **Rust** (latest stable), **SoapySDR** with drivers for your SDR.

```bash
# Install SoapySDR (Ubuntu/Debian)
sudo apt install libsoapysdr-dev soapysdr-tools

# Install driver for your SDR (example for LimeSDR)
sudo apt install soapysdr-module-lms7

# Build
cargo build --release
```

Binary output: `target/release/bluestation-bs`

---

## Configuration

```bash
cp example_config/config.toml ./config.toml
```

Key parameters added over upstream:

| Parameter | Default | Description |
|-----------|---------|-------------|
| `hangtime_secs` | `5` | How long a group call circuit stays open after floor release (seconds) |
| `call_timeout_secs` | `120` | Maximum active call duration before forced D-RELEASE (seconds) |
| `ul_inactivity_secs` | `3` | UL inactivity timeout before BS forces TX-CEASED (seconds) |
| `neighbor_cell_broadcast` | `0` | Neighbor cell broadcast mode in D-MLE-SYNC (0=off, 2=broadcast only) |
| `[security] issi_whitelist` | `[]` | ISSI whitelist — empty means open network |

### Neighbor Cell Broadcast

```toml
neighbor_cell_broadcast = 2

[[cell_info.neighbor_cells_ca]]
cell_identifier_ca = 1          # unique ID in cluster (0-31)
cell_reselection_types_supported = 0
neighbor_cell_synchronized = false
cell_load_ca = 0
main_carrier_number = 1522
mcc = 204
mnc = 1337
location_area = 3
```

Up to 7 `[[cell_info.neighbor_cells_ca]]` blocks. Each must have a unique `cell_identifier_ca` and `main_carrier_number`.

### ISSI Whitelist

```toml
[security]
issi_whitelist = [2260571, 2260572, 2260575]
```

When populated, only listed ISSIs can register. All others receive `D-LOCATION-UPDATE-REJECT`.

---

## Documentation

Base documentation (hardware setup, detailed configuration, wiring) is maintained by upstream:

[https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki](https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki)

---

## Credits

- **Harald Welte** and the **osmocom** team for the initial work on osmocom-tetra, without which this project would not have existed.
- **Tatu Peltola** for extending rust-soapysdr with the timestamping functionality required for robust rx/tx, and for providing a native Rust Viterbi encoder/decoder used in the LMAC.
- The **MidnightBlueLabs** team for tetra-bluestation, the foundation on which FlowStation is built.
- **Stichting NLnet**, which allocated part of the [RETETRA3 project grant](https://nlnet.nl/project/RETETRA3/) for FOSS TETRA software implementation.

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
