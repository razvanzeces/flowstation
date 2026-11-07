# Multi-cell (N SDRs, one station) — implementation plan

Status: **implemented in v0.5.0**, not yet validated on air — see
[`multi-cell-status.md`](multi-cell-status.md). Goal: one `bluestation-bs` process drives N SDRs, each SDR
being one TETRA cell (optionally dual-carrier), sharing one subscriber database, one set of network
links (Brew / Asterisk / LST / DAPNET / Telegram) and one dashboard, with group calls spanning cells
and radios roaming between cells.

## Where we start from

| Area | Today | Blocker for N cells |
|---|---|---|
| Process layout | `build_bs_stack` builds one entity chain on one `MessageRouter` | Router is keyed by `TetraEntity` → only one `Umac`, `Mm`, `Cmce`… per router |
| PHY | `try_attach_phy` opens one `RxTxDevSoapySdr` | One device, one sample clock, one TDMA timebase |
| Config | One `[cell_info]`, one `[phy_io.soapysdr]` | No notion of "cell id" |
| Mutable state | `SharedConfig.state` (`StackState`) holds `SubscriberRegistry` **and** `timeslot_alloc` | Registry is fine to share; timeslot allocator is per-cell resource |
| Globals | `rf_status`, `health::registry`, `service_control`, `DETECTED_SDR_NAME`, dashboard caches | Single-valued; must become per-cell or aggregated |
| Mobility | `neighbor_cells_ca` broadcast in D-NWRK-BROADCAST; D-NEW-CELL PDU exists | No inter-cell registration / call handling |
| systemd | `ExecStartPre` resets **all** USB devices | Fine for one process (resets all SDRs together) |

## Architecture target

```
                     ┌──────────────── Site core (1 per process) ────────────────┐
                     │ SubscriberRegistry · Group→cells map · Call switch (CMCE-X)│
                     │ Brew · Asterisk · LST · DAPNET · Telegram · Dashboard      │
                     └───────▲──────────────────▲──────────────────▲──────────────┘
                   site bus  │ (crossbeam chans)│                  │
     ┌───────────────────────┴──┐  ┌────────────┴─────────────┐  ┌─┴──── … cell N
     │ Cell 0 thread            │  │ Cell 1 thread            │
     │ Router: PHY→LMAC→UMAC→   │  │ Router: PHY→LMAC→UMAC→   │
     │ LLC→MLE→MM→CMCE→SNDCP    │  │ LLC→MLE→MM→CMCE→SNDCP    │
     │ SDR #0 (serial A)        │  │ SDR #1 (serial B)        │
     └──────────────────────────┘  └──────────────────────────┘
```

Key decision: **one router + one RT thread per cell**, not one router for all cells. Each SDR has its own
clock and TDMA timebase; keeping them in separate loops avoids cross-cell timing coupling and keeps the
existing entity code (which assumes one of each entity) almost untouched. Cells talk to the site core
over bounded channels, never share `&mut` state.

---

## Phase 0 — Groundwork & measurements 
- Benchmark CPU of one cell on Pi 4/5 (per-thread %), decide supported max (likely 2 on Pi 4, 3–4 on Pi 5).
- Test two SDRs on one USB bus (LimeSDR Mini 2 + SXceiver) for throughput/underruns.
- Inventory every `static`/`OnceLock` and every `cfg.config().cell` / `phy_io` use (14 files) → tag
  each as *per-cell* or *site-wide*.
- Add `CellId(u8)` type in `tetra-core`.

**Exit:** written inventory + CPU budget; no behaviour change.

## Phase 1 — Config schema 
- New optional `[[cells]]` array; each entry: `id`, `cell_info` (carriers, colour code, LA, BS id…),
  `soapysdr` (`device` serial mandatory when >1 cell, gains, fs, centers).
- Legacy single `[cell_info]` + `[phy_io.soapysdr]` auto-maps to `cells = [{ id = 0, … }]` — existing
  configs keep working unchanged.
- Validation: unique ids, unique device serials, no overlapping carriers, same MCC/MNC, distinct
  (LA or colour code) per cell, each cell passes the existing passband check.
- `StackConfig::cell(id)` accessor; `SharedConfig` gains a per-cell view (`CellConfig`) that entities
  use instead of `config().cell`.

**Exit:** parser + validator + unit tests; stack still runs only `cells[0]`.

## Phase 2 — Per-cell stack instantiation 
- Refactor `build_bs_stack` → `build_cell_stack(cell_id, …) -> CellRuntime` (router + entities + PHY).
- Move `timeslot_alloc` from `StackState` into per-cell state.
- Spawn one RT thread per cell (FIFO priority as today), each opening its SDR by serial.
- Globals: `rf_status` and `health::registry` become maps keyed by `CellId`, with an aggregate view
  (site "online" = all/any cells online, configurable). `service_control` stays site-wide.
- With N=2 and **no** inter-cell features: two cells run, each only serves its own radios; network
  entities (Brew etc.) still attached to cell 0 only.

**As built:** instead of moving `timeslot_alloc` now, each extra cell gets its own `SharedConfig`
(from `StackConfig::for_extra_cell`, network services stripped), so state is fully separate until
Phase 3 shares the registry. A thread-local `cell_context` marks each cell thread; health gauges and
the detected-SDR badge stay primary-only, and `rf_status::get_all()` reports every cell. A cell
whose SDR fails to open is not started (no PHY = nothing paces its loop).

**Exit:** two SDRs transmitting two independent cells from one process; single-cell configs unchanged
(regression test: existing integration tests in `crates/tetra-entities/tests` pass).

## Phase 3 — Site core & site bus 
- Extract Brew / Asterisk / LST / DAPNET / Telegram / GeoAlarm out of the per-cell router into a
  site-core thread. Define `SiteMsg` (voice frames, SDS, call control events, registration events)
  with `CellId` tagging.
- `SubscriberRegistry` gains `current_cell: Option<CellId>` per ISSI; MM on each cell writes it on
  U-LOCATION-UPDATE.
- Site core builds `group → set<CellId>` from affiliations.
- Routing rules: incoming Brew/LST group call → every cell with members of that group; SDS to ISSI →
  the cell holding it; unknown location → broadcast/page all cells.

**As built:** no separate site-core thread. `net_site::SiteSwitch` wraps the Brew/LST entity in the
primary router; each extra cell has a `CellLink` in its Brew slot (crossbeam channels). The switch
learns ISSI location / group membership from the `MmSubscriberUpdate`s cells send to the network,
routes by carrier (unique per cell), Brew UUID, or destination, fans network group calls out (first
`NetworkCallReady` is the anchor; its DL voice is copied to the other cells' circuits; Hold only
when every cell holds; `GroupListenersAvailable` pulls a cell into a running call), delivers SDS
between cells directly, and renumbers call ids. Each cell still keeps its own `StackState`, so the
dashboard/registry view is primary-only until Phase 6. Requires Brew or LST; without a network
link cells stay independent. Asterisk stays primary-only.

**Exit:** external network traffic reaches radios on any cell; SDS works across cells.

## Phase 4 — Inter-cell group & individual calls 
- CMCE "call switch" in site core: a group call started on cell A triggers D-SETUP on every other cell
  with members; UL voice from the talker's cell is fanned out as DL TCH to other cells (and to Brew).
- Floor control (U-TX-DEMAND / D-TX-GRANTED) arbitrated centrally so only one talker site-wide.
- Individual (P2P/duplex) calls between radios on different cells.
- Handle unequal TDMA timing between cells: voice is re-framed per cell (jitter buffer similar to
  `net_brew/components/jitter_buffer.rs`).

**As built:** all in `SiteSwitch`, no CMCE call-switch. A radio's `FloorGranted` creates a local
session: the other cells with members get a `NetworkCallStart` (they treat the talker as a network
speaker) and its `TmdCircuitDataInd` uplink is copied to their circuits; `FloorReleased` sends them
`NetworkCallEnd` (hangtime), from which any cell's radio can take the floor. A grant on another
cell while someone talks loses: its cell is pulled into the call and its floor events never reach
the network. Individual calls to a radio on a sibling cell are connected by passing the circuit-call
signalling across (CMCE's in/out variants are symmetric) and copying voice both ways. "Site-linked"
mode (`StackConfig::is_site_linked`): cells report every registration/floor/call to the slot and
accept every inbound group call; the switch re-applies the Brew rules towards the real network,
and mirrors `network_connected` / `brew_link_up` from the primary to the other cells (this also
fixes Phase 3 cells advertising "not connected"). Idle sessions are purged after 10 min.
Not done: no jitter buffer on copied voice (frames go straight into the other cell's next tick);
priority/emergency is not carried on cross-cell local calls (sent as priority 0).

**Exit:** radio on cell 0 and radio on cell 1 in the same talkgroup hear each other; P2P works.

## Phase 5 — Mobility 
- Auto-populate `neighbor_cells_ca` from sibling cells (no manual config).
- Cell reselection: accept migrating U-LOCATION-UPDATE, move ISSI in registry, silently drop the stale
  entry on the old cell (MM state cleanup).
- Call restoration across cells (U-CALL-RESTORE / D-CALL-RESTORE already partly in
  `cc_bs/procedures/restoration.rs`) so an ongoing group call survives a cell change.
- Optional: announced handover via D-NEW-CELL (stretch goal; many terminals do fine with
  unannounced reselection + restore).

**As built:** `StackConfig::add_sibling_neighbours` (called at startup) adds every sibling cell to
each cell's `neighbor_cells_ca` (configured entries on the same carrier are kept, 7-entry limit,
first free `cell_identifier_ca`, LA only if it differs) and sets the D-NWRK-BROADCAST-supported bit
in `neighbor_cell_broadcast`. When the switch sees a radio register on a new cell it sends the old
cell's MM a Brew-sourced `Deregister`: MM drops the registration without anything on air (the
dashboard "kick" path sends D-LOCATION-UPDATE-COMMAND instead), CMCE drops its listener counts so
the old cell releases group calls nobody hears any more; the old cell's resulting deregister /
deaffiliate never reaches the network. In site-linked mode CMCE matches a U-CALL RESTORE with an
unknown call id (a sibling cell's) to the active group call of its `other_party_ssi` GSSI and
answers with its own call id. D-NEW-CELL / announced handover: not done (stretch goal); individual
calls do not survive a cell change (the peer is released as before).

**Exit:** walking a radio between two cells keeps registration and rejoins an active call.

## Phase 6 — Dashboard, ops & packaging 
- Dashboard: cell selector / per-cell cards (RF status, carriers, load, registered radios, SDR name),
  site-wide views remain aggregated. `/api/btsinfo` returns a `cells` array.
- Config UI: add/remove cell, pick SDR by detected serial (Setup wizard enumerates all Soapy devices).
- Telemetry/control protocol: add `cell_id` to events.
- systemd: keep one unit; USB reset stays "all devices" (acceptable since one process owns all SDRs).
- Docs + CHANGELOG, beta release, then promote to stable.

---

**Phase 6 as built:** `GET /api/cells` (every cell: carriers, CC, LA, SDR, RF state from
`rf_status::get_all`, registered radios from each cell's `StackState` via
`net_site::register_extra_cells`), `POST /api/cells/add` / `remove` (text edit of `[[cells]]`,
prospective config parsed + validated before an atomic write, `.cells.bak` backup, restart). Home
page **Cells** card lists the cells, scans SDRs (existing `scan-sdr`) and adds/removes cells;
EN + ES strings. Not done: per-cell telemetry `cell_id` (extra cells have no telemetry sink), the
registered-radios table and Setup wizard remain primary-cell views. TMO profiles keep `[[cells]]`
(they deep-merge into the file) but don't edit them; a profile whose carrier or SDR clashes with a
cell fails the usual config validation.

**Follow-up fixes:** copied voice goes through a per-circuit `VoiceJitterBuffer` at the
receiving cell (`VoicePlayout` in `CellLink` / `SiteSwitch`) and plays out on that cell's own
timeslot, one frame per slot, none in frame 18. Priority: a site-linked CMCE sends
`CallControl::SiteCallPriority` (switch-only, never forwarded) before a raised-priority
`FloorGranted`; the switch carries it in the other cells' `NetworkCallStart`, and a higher-priority
talker pre-empts one on another cell. Telemetry: extra cells use the station's sink via
`TelemetrySink::for_cell` (registrations followed by `MsCell { issi, cell }`, appended last for
bitcode wire-stability; extra cells' call events get station-wide ids from 0x4000 up, shared by
all the stream's sinks, since CMCE call ids are per cell and ≤ 0x3FFF);
the dashboard shows the cell per radio. MM's silent move cleanup uses `remove_client_quiet` so a
moved radio doesn't disappear from the table.

**Announced handover (D-NEW-CELL) as built:** MLE BS now actually handles uplink MLE PDUs (the
MLE branch used to read an SDU that had already been taken, and decoded downlink types, so every
uplink MLE PDU was dropped). The U-PREPARE / U-RESTORE / D-NEW-CELL / D-PREPARE-FAIL /
D-RESTORE-ACK / D-RESTORE-FAIL codecs are implemented (the "SDU" element has no P-bit and runs to
the end of the PDU). U-PREPARE to an advertised neighbour (`cell_identifier_ca`) gets D-NEW-CELL
(channel command valid = change channel immediately), else D-PREPARE-FAIL; site-linked cells also
send the switch `CallControl::SiteHandoverPrepare`, and the switch registers the MS as a listener of
its groups in the target cell's CMCE (so that cell joins its group calls via
`GroupListenersAvailable`), undone after 30 s if the MS never registers there. U-RESTORE hands its
U-CALL RESTORE to CMCE; MLE wraps CMCE's D-CALL RESTORE in D-RESTORE-ACK or turns its D-RELEASE into
D-RESTORE-FAIL. Type 1 forward registration: the MM PDU inside U-PREPARE goes via the switch
(`SiteForwardRegistration`) to the target cell's MLE, which hands it to its MM and captures MM's
answer (instead of transmitting it there) as `SiteForwardRegistrationResult`; the serving cell holds
D-NEW-CELL until then and sends it with that answer as SDU (D-PREPARE-FAIL if it is a D-LOCATION
UPDATE REJECT), or without it after 3 s. `cell_reselection_types_supported` in auto neighbours
stays 1 — check with real terminals.

**Asterisk and WX on every cell:** `StackConfig::cell_id` / `is_multi_cell()` mark a per-cell
config. Linked extra cells keep `asterisk.enabled`; their router gets a second `CellLink` in the
`Asterisk` slot (`CellLink::asterisk_link`) feeding a separate channel, and the primary's
`AsteriskEntity` is wrapped in `net_site::SiteRelay`, which routes its output by carrier (voice,
re-timed through the receiving cell's playout buffer), by the radio's location for a SIP call to a
radio (directory now `SharedDirectory`, shared with the switch), and by session UUID otherwise. WX
runs on every cell with its own CMCE command link for replies; periodic WX to an ISSI is sent only
by the cell the ISSI is registered on (to a group, by each cell to its members); dashboard WX
overrides are written to every cell's state. Also fixed: group SDS reached other cells only when the
sender's cell had no members and Brew SDS was on — linked CMCEs now always hand non-local and local
group SDS to the switch, which sends to the network only an SDS for nobody on site (and only with
Brew SDS on).

**Reselection:** auto sibling neighbours advertise `cell_reselection_types_supported = 3` (TS 100
392-2 table 18.22: forward registration supported, expedited cell reselection recommended), the main carrier number extension (band 4 | offset 2 | duplex 3 | reverse 1 bits)
when the sibling's band plan differs, and its `ms_txpwr_max_cell` when it differs; cells must share
`custom_duplex_spacing` (not expressible to radios). D-NWRK-BROADCAST with neighbours now sends
`[cell_info.cell_reselect]` (slow/fast threshold and hysteresis, 4 bits each in 2 dB units; default
20/10/10/6 dB) instead of 0; the time-only broadcast keeps 0. Nibble order and defaults are from my
reading of clause 18.5 — verify with a terminal's field-test display.

## Risks
- **CPU on Raspberry Pi** — may cap practical N at 2; Phase 0 decides.
- **USB bandwidth / power** for two SDRs on one Pi (powered hub may be required).
- **Terminal behaviour** on reselection/restoration differs per vendor (Motorola MXP600/MTM800E/MTM5400
  must be tested each phase).
- **Regression of single-cell users** — every phase must keep legacy config working; gate multi-cell
  behind `[[cells]]` presence.
- **Panic containment**: one cell's caught panic must not degrade others (per-cell health counters).

