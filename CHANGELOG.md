# FlowStation Changelog

All notable changes relative to the upstream [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation) fork point (v0.5.9) are documented here.

## [0.1.0] — 2026-05-01

### Added — Full-duplex individual calls

- Ported `cc_bs` duplex call module from `tetra-bluestation-feature-duplex-calls-brew` branch
- Full-duplex P2P calls local (MS↔MS on same BS) — ETSI EN 300 392-2 §14 compliant
- Full-duplex individual calls over Brew/TetraPack (circuit-switched)
- DTMF forwarding (U-INFO → Brew) for Brew-routed individual calls
- `IndividualCall`, `IndividualCallState` with ETSI timers T301/T302/T310
- `allocate_circuit_with_allocator_duplex()` in `circuit_mgr`
- Brew protocol extensions: `SETUP_REQUEST`, `CONNECT_REQUEST`, `CALL_ALERT`, `CONNECT_CONFIRM`, `CALL_RELEASE`, `DTMF_FRAME`

### Added — Configurable timers (all in `[cell_info]` section of config)

| Parameter | Default | Range | Description |
|-----------|---------|-------|-------------|
| `hangtime_secs` | 5 | 0–300 | Group call floor idle time before teardown |
| `call_timeout_secs` | 120 | 30–300 | Max call duration (ETSI T310 equivalent) |
| `ul_inactivity_secs` | 3 | 1–30 | UL silence before forced TX-CEASED |

### Fixed — Stability

- **FACCH steal on closed circuit** (`umac_bs.rs`): D-RELEASE no longer attempts STCH stealing after circuit is closed — falls back to MCCH silently
- **DL voice on inactive circuit** (`net_brew/entity.rs`): jitter buffer flushed immediately on `drop_network_circuit`, preventing 11k+ dropped frame warnings
- **Speaker change audio bleed** (`net_brew/entity.rs`): jitter buffer flushed on GROUP_TX speaker change per ETSI §14.8.43
- **Brew disconnect** (`net_brew/entity.rs`): all active calls released immediately on backhaul loss with `WARN` level log; per ETSI §14.9.4
- **`blk2_stolen` panic** (`lmac_bs.rs`): `assert!` replaced with graceful `warn` + reset + return — no more crash on late STCH after circuit teardown
- **Stale circuit cleanup** (`timers.rs`): `SendClose` now logs clearly and performs full cleanup even without cached D-SETUP
- **Call timeout** (`fsm/setup.rs`, `fsm/network.rs`, `fsm/individual.rs`, `shared.rs`): hardcoded `T5m` replaced with `config_call_timeout()` reading from `call_timeout_secs`
- **UL inactivity** (`umac_bs.rs`): hardcoded 3s constant replaced with `ul_inactivity_secs` from config

### Fixed — Log noise

- `identify_timeslots_for_ssi` WARN → TRACE (fires on every MCCH message)
- `defrag_buffer not inactive` WARN → DEBUG (normal behavior under RF loss)
- `D-SETUP resend skip` DEBUG → TRACE (fires every timer tick for Brew calls)
- `blk2_stolen` panic → graceful WARN

### Changed

- Project renamed: **BlueStation → FlowStation**
- Version reset to `0.1.0` (new fork baseline)
- Binary renamed: `bluestation-bs` → `flowstation-bs`
- License: AGPL-3.0 (unchanged from upstream)
- Authors: FlowStation Contributors (upstream: Wouter Bokslag / Midnight Blue)
