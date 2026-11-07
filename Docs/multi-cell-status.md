# Multi-cell — status

_Last updated: 2026-09-30, release v0.5.0 (branch `bost`)._

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
  number extension (band 4 | offset 2 | duplex 3 | reverse 1), D-NEW-CELL channel command valid = 1,
  fail causes sent as 0, `cell_reselection_types_supported` = 3 meaning "both".
- Voice between cells: playout buffer depth / latency (reuses Brew's `VoiceJitterBuffer`).
- Hangtime handovers between cells (a cell's CMCE handling a network call start while its own call
  hangs, and a local PTT during a network call).

## Several networks per station

Cells can now be bound to different networks (`[networks]`, `Docs/networks.md`): each network has
its own identity and core (or none), its own site switch hosted by its home cell, and radios move
between networks by selecting one on the terminal. Not yet validated on air.

## Known limits (by design, documented)

- Linking needs Brew or LST; without them each cell runs alone (Asterisk then primary-only).
- Individual calls don't survive a change of cell.
- A group call spanning cells shows once per cell in the Calls list.
- Only MM's first answer to a forwarded registration is captured; follow-up MM PDUs go on air on
  the target cell.
- CPU / bandwidth with several SDRs on one Pi untested; start with two cells.

## Housekeeping

- The first workstation has a local git stash on `beta` ("multi-cell phase 1 (from beta d3c7f00)")
  and an unpushed `beta` commit `ad4a60c` (test fix). Both are local only and superseded by `bost`;
  nothing needs to be carried over.
- No `v0.5.0` git tag or GitHub release has been created.
