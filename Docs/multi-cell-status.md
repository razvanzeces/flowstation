# Multi-cell — status

_Last updated: 2026-11-06, release v0.5.6 (milestone 7 work on branch `handover` of the fork)._

## Done (in code, with unit/integration tests)

| Area | What | Commit(s) |
|---|---|---|
| Plan | `Docs/multi-cell-plan.md` | `6028653` |
| Phase 1 | `[[cells]]` config, `CellId`, cross-cell validation | `e250e25` |
| Phase 2 | One radio stack + SDR thread per cell | `2924daf` |
| Phase 3 | Site switch: Brew/LST shared, network calls/SDS routed per cell | `1a61222` |
| Phase 4 | Calls between cells, one talker per group, site-linked mode | `c949a16` |
| Phase 5 | Auto neighbours, silent cleanup on move, restore by GSSI | `da7f626` |
| Phase 6 | Dashboard Cells card, add/remove cell API | `44be7e4` |
| Fixes | Voice playout buffer, call priority, per-cell telemetry | `7ec293d`, `2a39777` |
| Handover | MLE uplink PDUs, D-NEW-CELL / D-RESTORE-*, target-cell prep | `82fc967` |
| Asterisk/WX | SiteRelay, WX on every cell, group SDS across cells | `65c38e9` |
| Type 1 | Forward registration inside U-PREPARE → D-NEW-CELL | `5ac4f6a` |
| Reselection | Types = 3, carrier extension, max power, `[cell_info.cell_reselect]` | `268bf79` |
| Release | README, CHANGELOG, version 0.5.0 | `6079855`, `c980acf`, `fcb0b7e` |
| Dashboard | BTS Details shows every cell + carrier (timeslots from `/api/cells`); RF page Cells card. Not yet in CHANGELOG / no version bump | `5df20cd` |
| Individual calls (PR #30) | Individual call handover by David Harcus: leg pre-provisioned on an announced change, matched by ISSI pair | `0b053a4` (upstream) |
| Network calls across cells | Calls with a Brew party follow the radio to the other cell (`restore_from_sibling`), announced or not | `c7630d7` |
| Location areas | Shared or own LA per cell, paging on every cell of the LA, presence check on all of them, LA field | `881e64d` |
| Roaming page | Page "Infrastructure" (formerly "Roaming / Cells"), neighbours per cell, server cell list (`cell-directory`) | `52d1171`, `9779d99`, `2e6b19c` |
| Roaming with stations | A multi-cell station takes part in roaming between stations over Brew (type 2) | `543c2a8` |
| Radio ↔ radio across cells | Calls between two radios survive a change of cell: hand over through the switch, join on the peer's cell | `55a0881` |

## Not validated — next step is an on-air test

Nothing multi-cell has run on real SDRs or radios yet; Asterisk has only been type-checked (it needs
the native codec). Suggested test with two Pluto+ and Brew enabled, in this order:

1. Both cells come up (`cell1: cell stack running` in the log; Cells card shows both RF online).
2. Radios on each cell in one talkgroup: talk on one, hear on the other; answer during hangtime.
3. Both press PTT together → only one gets through. Emergency call takes the floor.
4. Individual call between cells; SDS between cells (ISSI and group).
5. Walk a radio between cells (or lower one Pluto's TX gain): stays registered, keeps its group call.
   Log: `SiteSwitch: ISSI … moved cell0 → cell1`. With a terminal that supports it: announced
   handover (`MLE: U-PREPARE … → D-NEW-CELL`).
6. Radio field-test display: both cells listed as neighbours; reselection thresholds read
   20/10/10/6 dB.
7. If used: PBX calls to/from radios on cell 1; WX request on cell 1.

## Most likely to need correction after testing

- Bit layout from our reading of EN 300 392-2 (verify on a terminal): the 16-bit cell re-select
  parameters (slow thr | fast thr | slow hyst | fast hyst, 2 dB units), the neighbour main carrier
  number extension (band 4 | offset 2 | duplex 3 | reverse 1). Checked against TS 100 392-2 V3.10.1:
  D-NEW-CELL channel command valid 1 = change channel immediately (18.5.6); D-RESTORE-FAIL fail cause
  3 = restoration cannot be done on cell (18.3.4.7, 18.5.7); D-PREPARE-FAIL fail cause 0 = temporary
  break in service; `cell_reselection_types_supported` (table 18.22) means 0 forward registration not
  supported, 1 not supported + expedited cell reselection recommended, 2 supported, 3 supported +
  expedited recommended. It does not select announced or unannounced reselection; the auto
  neighbours' 3 means "forward registration supported, expedited recommended".
- Voice between cells: playout buffer depth / latency (reuses Brew's `VoiceJitterBuffer`).
- Hangtime handovers between cells (a cell's CMCE handling a network call start while its own call
  hangs, and a local PTT during a network call).

## Several networks per station

Cells can now be bound to different networks (`[networks]`, `Docs/networks.md`): each network has
its own identity and core (or none), its own site switch hosted by its home cell, and radios move
between networks by selecting one on the terminal. Not yet validated on air.

## Known limits (by design, documented)

- Linking needs Brew or LST; without them each cell runs alone (Asterisk then primary-only).
- Individual calls survive a change of cell, announced or not. With a Brew party the switch answers the new cell's `IndividualCallRestoreRequest` (`net_site/switch.rs` `restore_from_sibling`). Between two radios of the station the switch asks the cell holding the local call to hand it over (`ask_sibling_for_local_call`), and a radio coming to its peer's cell is joined there (CMCE `join_network_call_locally`). With three or more cells, a leg pre-provisioned by PR #30 that is never used keeps its timeslot until the call timeout.
- A group call spanning cells shows once per cell in the Calls list.
- Cells sharing a location area: group calls run on every cell of a member's area (`SiteAreaMembers` to the sibling CMCEs) and presence checks are repeated there (`SitePresenceProbe` → `MmRequestLocationUpdate`); network individual call setups are paged on every cell of the area (`CircuitCall.paging`, first cell to alert/connect wins, the others get a silent release); an individual SDS or a radio-to-radio call to an idle radio still goes to the cell it last registered on until it transmits or answers a presence check.
- Extra cells may have their own neighbour list (`[[cells.cell_info.neighbor_cells_ca]]`); without one they advertise only the siblings. Dashboard page Roaming (`net_dashboard/roaming.rs`, `/api/roaming`).
- Roaming with other stations over Brew (`handover-v1`, `ms-moved`, `call-restore`) works as on a single-cell station: the site switch routes a forwarded registration to the cell of its target carrier (`station_forwards`), the answer to a cell's request back to that cell (`station_asks`), an `ms-moved` drop to the radio's cell only, and holds an individual call restored from another station on the asking cell. MLE asks Brew for a neighbour that is not one of the station's carriers (`StackConfig::is_station_carrier`). Type 2 only between stations (no type 1).
- Only MM's first answer to a forwarded registration is captured; follow-up MM PDUs go on air on
  the target cell.
- CPU / bandwidth with several SDRs on one Pi untested; start with two cells.

## Housekeeping

- The first workstation has a local git stash on `beta` ("multi-cell phase 1 (from beta d3c7f00)")
  and an unpushed `beta` commit `ad4a60c` (test fix). Both are local only and superseded by `bost`;
  nothing needs to be carried over.
- No `v0.5.0` git tag or GitHub release has been created.
