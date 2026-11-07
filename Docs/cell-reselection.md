# Cell reselection tuning (`rxlev_access_min_dbm`, `cell_reselect`)

When a radio leaves one cell for another is decided by the radio, not by the station. The
station only broadcasts the parameters the radio uses for that decision. This page explains the
two config parts that set them and how they work together. References are to ETSI TS 100 392-2
V3.10.1.

Both parts are optional. Without them the station starts as before: −110 dBm and reselection
values 20/10/10/6 dB.

## The parameters

### `[cell_info] rxlev_access_min_dbm`

```toml
[cell_info]
rxlev_access_min_dbm = -80
```

- Sent in SYSINFO as RXLEV_ACCESS_MIN (table 21.65): −125 to −50 dBm in 5 dB steps.
  Any other value, e.g. −77, is rejected at startup with an error naming the key.
- Absent: −110 dBm.
- Extra cells (`[[cells]]`) inherit it from `[cell_info]` and may override it in
  `[cells.cell_info]`. If a sibling cell has another level, it is advertised to radios as that
  neighbour's "minimum RX access level" (clause 18.5.12).
- For a neighbour on another station listed in `[[cell_info.neighbor_cells_ca]]`,
  `minimum_rx_access_level` (0–15, same coding: −125 + 5 × value dBm) may be set. If it is
  absent, the radio assumes the neighbour uses the same value as the serving cell (table 18.64,
  note 2). Use the same level on all stations, or set it per neighbour.

### `[cell_info.cell_reselect]`

```toml
[cell_info.cell_reselect]
slow_threshold_db = 20     # SLOW_RESELECT_THRESHOLD_ABOVE_FAST
fast_threshold_db = 10     # FAST_RESELECT_THRESHOLD
slow_hysteresis_db = 10    # SLOW_RESELECT_HYSTERESIS
fast_hysteresis_db = 6     # FAST_RESELECT_HYSTERESIS
```

- Sent in D-NWRK-BROADCAST with the neighbour list (clause 18.5.4, table 18.38), so only when
  neighbours are configured or the station runs several cells. 0–30 dB in 2 dB steps.
- `slow_threshold_db` is **added to** the fast threshold: SLOW_RESELECT_THRESHOLD =
  FAST_RESELECT_THRESHOLD + SLOW_RESELECT_THRESHOLD_ABOVE_FAST (clause 18.5.4). With the
  defaults the slow threshold is 10 + 20 = 30 dB.

## How the radio uses them

The radio computes the path loss parameter C1 for the serving cell and for each neighbour it
scans (clause 23.7.1.1, equation 23.12):

```
C1 = RSSI − RXLEV_ACCESS_MIN − max(0, MS_TXPWR_MAX_CELL − PMSMAX)
```

The last term is 0 when the radio can transmit at least `ms_txpwr_max_cell`. Then C1 is simply
"how many dB the received level is above `rxlev_access_min_dbm`". C1 is compared against the
thresholds:

| Condition | Clause | Needs |
| --- | --- | --- |
| Radio link failure | 18.3.4.5.3 | C1 < 0 |
| Serving cell **relinquishable** | 18.3.4.5.4 | serving C1 below FAST_RESELECT_THRESHOLD for 5 s, and a neighbour above serving by FAST_RESELECT_HYSTERESIS for 5 s |
| Serving cell **improvable** | 18.3.4.5.5 | serving C1 below SLOW_RESELECT_THRESHOLD for 5 s, and a neighbour above serving by SLOW_RESELECT_HYSTERESIS for 5 s |
| Neighbour **usable** | 18.3.4.5.6 | neighbour C1 above FAST_RESELECT_THRESHOLD + FAST_RESELECT_HYSTERESIS for 5 s |

Both criteria also need 15 s since the last successful reselection, unless MM asks for one.

When the radio starts monitoring neighbours at all is left to the radio (clause 18.3.4.5.1,
"outside the scope of the present document"). A radio that monitors only below its own threshold
may not see a better neighbour until the serving cell is weak.

Whether the radio announces the change (U-PREPARE, D-NEW-CELL) or changes unannounced is also
the radio's choice within what the neighbour advertises (clause 18.3.4.7). A radio that only
listens to a group call has no need to announce (clause 18.3.4.7.0).

## `cell_reselection_types_supported` (neighbour entries)

Per neighbour in `[[cell_info.neighbor_cells_ca]]`, 2 bits, table 18.22. It does **not** choose
announced or unannounced reselection; that is always the radio's choice.

| Value | Forward registration | Expedited cell reselection |
| --- | --- | --- |
| 0 | not supported | not recommended |
| 1 | not supported | recommended |
| 2 | supported | not recommended |
| 3 | supported | recommended |

- **Forward registration** (18.3.4.7.1): the radio may send its registration for the new cell
  in U-PREPARE on the old cell, so it arrives registered. Only useful when the old station can
  pass it on (multi-cell siblings, or the cross-station handover through Brew).
- **Expedited reselection** (18.3.4.5.6b): the neighbour becomes a candidate when it is better
  by FAST_RESELECT_HYSTERESIS for 2 s (instead of 5 s), and the radio should monitor it at the
  fastest rate and with less filtering (18.3.4.5.1 and note 1 of 18.3.4.5.6b). Two side effects:
  - The radio does not hold back the change during a call (see below).
  - The radio **shall not** attempt announced type 1/2 reselection to a cell that recommends
    expedited reselection (18.3.4.7.5). So 1 and 3 mean unannounced changes only.

Multi-cell sibling neighbours are advertised with 3.

## Changing cell during a call

If the neighbour does not recommend expedited reselection and the radio is in a circuit mode
call or in ongoing signalling, the change "should be postponed ... until the serving cell becomes
radio relinquishable, even if there are neighbour cells which meet the radio improvable or radio
usable criteria" (18.3.4.5.7). In a call only the fast threshold counts: with the defaults and
−110 dBm, a radio listening to a group call stays until about −100 dBm.

## In dBm

With the radio's term at 0, each threshold is a received level at the radio:

| | Defaults, −110 dBm | Defaults, −80 dBm |
| --- | --- | --- |
| Radio link failure | below −110 | below −80 |
| Relinquishable (fast, 10 dB) | below −100 | below −70 |
| Improvable (slow, 10 + 20 = 30 dB) | below −80 | below −50 |

Raising `rxlev_access_min_dbm` moves all thresholds up by the same amount. The radio starts
looking for a better cell while the serving cell is still good. The cost is range: below the
level the radio declares radio link failure and cannot access the cell at all (table 18.56,
note 3: "the BS may use the minimum RX access level to limit the range of the cell").

## Choosing values

- Single station, no neighbours: leave both parts out.
- Two or more stations or cells with overlapping coverage, radios that stay on the old cell until
  the signal is gone: raise `rxlev_access_min_dbm` in 5 dB steps on all of them (for example
  −110 → −90 → −80) and keep the `cell_reselect` defaults. Check at the edge of the coverage
  that radios still get in.
- Keep the hysteresis large enough (defaults 10/6 dB). Small hysteresis with high thresholds lets
  a radio between two cells change back and forth.
- A large `slow_threshold_db` adds to the fast threshold: 30/20 gives a slow threshold of 50 dB,
  which at −80 dBm means "improvable" below −30 dBm, practically always.

## Checking

- Dashboard Cell Change Log: which cell a radio moved to and whether it was announced.
- Station log: `U-PREPARE` (announced) or a registration or `U-RESTORE` on the new cell without
  one (unannounced).
- Many radios show C1 or RSSI of the serving cell and the neighbours in a field-test menu.
