# Several networks in one station (`[networks]`)

A station can serve more than one TETRA network at once. Each network has its own identity
(MCC/MNC) and either its own Brew core or none, and every cell is bound to one network. A radio
user who selects a network on the terminal lands on that network's cells and is routed to that
network's core; selecting another network moves the radio to the other network's cells. Nothing
crosses between networks inside the station.

Typical uses:

- One carrier on TetraPack (MNI 901/9999) and one on another core (say 901/1234), chosen from the
  radio's network list.
- A club network with no core at all: two or three cells switched inside the station, calls, SDS
  and roaming between them, no server anywhere.
- A public network plus a private one on the same mast, each with its own subscribers.

## Configuration

```toml
network = "tetrapack"              # the primary cell's network (required with [networks])

[networks.tetrapack]               # MCC/MNC default to [net_info]
[networks.tetrapack.brew]          # same keys as the classic [brew]
host = "core.tetrapack.online"
port = 443
tls = true
username = 1000001
password = "secret"

[networks.club]                    # no brew: switched inside the station
mcc = 901
mnc = 1

[[cells]]
id = 1
network = "club"
[cells.cell_info]
main_carrier = 1525

[[cells]]
id = 2
network = "club"
[cells.cell_info]
main_carrier = 1529
```

Rules the loader enforces:

- `network = "…"` must be at the top of the file (before the first table), naming the primary
  cell's network. It is required once `[networks]` is present; the station does not guess.
- A `[[cells]]` entry without `network` serves the primary cell's network.
- With `[networks]`, a root `[brew]` is refused: core links belong to the network they serve.
  `[lst_dispatch]` stays with the primary cell's network and cannot be combined with a network
  that has a Brew link.
- Every network needs its own MCC/MNC (two networks with one identity would be one network to the
  radios), and every network must have at least one cell.
- A network's `mcc`/`mnc` default to `[net_info]`, so the classic identity can simply become one
  of several.

Without a `[networks]` section nothing changes: `[net_info]` and `[brew]` describe the one
network, called `default`, and cells are linked only when Brew or LST is configured, as before.

## How it runs

- Each network with more than one cell gets its own site switch, with its own radio directory,
  one-talker-per-group floor and call numbering towards its core. The switch runs in the
  network's *home cell*: the primary when it serves the network, otherwise the network's
  lowest-numbered cell. A network of one cell with a core runs its Brew link in that cell alone.
- A network without a core is *local*: its home cell carries a sink where the core link would be,
  so the switch joins its cells (group calls, individual calls, SDS and roaming between them) and
  nothing leaves the station. Such cells advertise `system_wide_services` from `[cell_info]`.
- Sibling cells are advertised to radios as neighbours as before; a sibling on another network is
  announced with that network's MCC/MNC, so a radio that has both networks in its list finds the
  other cell by itself.
- The Asterisk bridge, LST dispatch, the DAPNET/Telegram/geo-alarm services and the dashboard run
  beside the primary stack and serve the primary cell's network.

## What the dashboard shows

- The **backhaul** indicator (dashboard sidebar, health tile, Telegram alerts, and the
  `BrewConnected` telemetry event) follows the primary cell's network, as it always did.
- The **Cells** cards show each cell's network as `club 901/1 (local)` or
  `tetrapack 901/9999 (core up|core down)`. The same appears in `/api/cells` and in the
  `CellsSnapshot` telemetry event as `network` and `link_up` on every cell, so brew-server and
  other consumers can tell the networks apart.
- The Setup page's Brew **profiles** apply to a root `[brew]` and are refused on a station with
  `[networks]`; edit the network's `brew` table in the configuration instead.

## Not yet

- Routing by the radio's *home* network on a single carrier (a migrating radio presents its home
  MNI in U-LOCATION UPDATE DEMAND). Today a cell serves one network and a radio reaches another
  network by moving to that network's cell. Home-MNI routing is the natural next step on top of
  this and would also need migration enabled on the cell and in the radios' codeplugs.
- Passing a radio's network to the core over Brew, so a dispatch console can see which network a
  radio belongs to.
- On-air validation: this has been exercised in unit and integration tests only. The first live
  check is two carriers on different MNIs, a radio with both networks programmed, and switching
  networks on the radio while the Cells card shows which cell it registers on.
