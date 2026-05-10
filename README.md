# FlowStation

**FlowStation** is a fork of [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) (MidnightBlueLabs), with critical bug fixes, stability improvements, and extended features — developed and maintained by **Razvan Zeces YO6RZV**.

Tested hardware: **LimeSDR Mini 2.0**, **MXP600**, **MTM800E**, **MTM5400**.

---

## What's different from upstream

### Critical fix — ExpiryOfTimer loop
`release_group_call` now notifies Brew with `NetworkCallEnd` when a network-initiated group call expires. Without this fix, Brew kept the call alive and kept sending `NetworkCallStart` with new speakers, generating a loop of thousands of `ExpiryOfTimer` releases and crashing the stack.

### Simplex P2P calls (half-duplex)
- `transmission_request_permission` correctly set to `false` in `D-CONNECT`, `D-CONNECT-ACK`, `D-TX-CEASED` and `D-TX-GRANTED` — fixes the "Not allowed to transmit" error on Motorola/Sepura terminals.
- On `U-TX-CEASED`, BS sends `D-TX-CEASED` to the speaker and `D-TX-GRANTED(Granted)` to the peer. Terminals that receive `GrantedToOtherUser` in `D-CONNECT` need an explicit `D-TX-GRANTED` to unlock the PTT button — `D-TX-CEASED` alone is not enough.

### Web dashboard (port 8080)
- Live view of registered terminals with RSSI, groups, status
- Active call monitoring with duration timer
- Kick terminal (forces immediate re-registration via `D-LOCATION-UPDATE-COMMAND`)
- Send SDS messages to any registered ISSI
- Live log with level filtering
- Config editor with save & restart
- Multi-language: EN, RO, DE, ES

### Periodic registration (T351)
Correct ETSI EN 300 392-2 implementation: on expiry, BS sends `D-LOCATION-UPDATE-REJECT` with cause `ExpiryOfTimer (17)` and type `PeriodicLocationUpdating`, then removes the terminal from the registry. The terminal re-attaches immediately.

---

## What works

| Feature | Status |
|---------|--------|
| Group calls (local) | ✅ |
| Group calls via Brew (BrandMeister / TetraPack) | ✅ |
| Full-duplex P2P calls (local + Brew) | ✅ |
| Half-duplex P2P calls (simplex PTT) | 🔧 in testing |
| SDS forwarding (local + Brew) | ✅ |
| UTC time broadcast (D-NWRK-BROADCAST) | ✅ |
| T351 periodic registration | ✅ |
| Web dashboard | ✅ |

---

## Installation

### From release (recommended)

```bash
# Download the latest release archive from the Releases page
tar -xzf flowstation-v*.tar.gz
cd tetra-bluestation
cp example_config/config.toml ./config.toml
# Edit config.toml for your setup
cargo build --release
```

### From git

```bash
git clone https://github.com/razvanzeces/flowstation.git tetra-bluestation
cd tetra-bluestation
cp example_config/config.toml ./config.toml
# Edit config.toml for your setup
cargo build --release
```

**Requirements:** Rust (latest stable), SoapySDR with drivers for your SDR.

The binary is at `target/release/bluestation-bs`.

---

## Configuration

Key parameters (new vs upstream):

| Parameter | Default | Description |
|-----------|---------|-------------|
| `hangtime_secs` | `5` | How long to hold a group call circuit after floor release |
| `call_timeout_secs` | `120` | Max call duration before forced D-RELEASE |
| `ul_inactivity_secs` | `3` | UL inactivity timeout before forced TX-CEASED |
| `periodic_registration_secs` | `0` | T351 interval in seconds; `0` = disabled |

Full configuration reference is maintained upstream:
[https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki](https://github.com/MidnightBlueLabs/tetra-bluestation-docs/wiki)

---

## Branches

| Branch | Purpose |
|--------|---------|
| `main` | Stable, tested releases |
| `beta` | Work in progress, new features |

---

## Credits

- **Harald Welte** and the **osmocom** team for the original osmocom-tetra work.
- **Tatu Peltola** for rust-soapysdr timestamping and the native Rust Viterbi encoder/decoder used in LMAC.
- **MidnightBlueLabs** for tetra-bluestation, the base this project builds on.
- **Stichting NLnet** for partially funding this work through the [RETETRA3 grant](https://nlnet.nl/project/RETETRA3/).

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
