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

## Reached through another network's Brew server (`via`)

A network can also be reached without credentials of its own on the station: `via` names the
station's network whose Brew server reaches it (the "Infrastructure" way of Multi Network, see
brew-server `docs/Multi-Network.md`). The network's cells broadcast its own MCC/MNC and connect
with that network's `[brew]` login, and tell the server, which must offer the `mni` extension,
that their radios and groups belong to this network (`SERVICE_MS_NETWORK`,
`SERVICE_GROUP_NETWORK`, `SERVICE_CALL_NETWORK`). The server registers them in that network, for
example over its link to TetraPack, and routes their calls and SDS there.

```toml
network = "home"

[networks.home]                    # 901/9999 from [net_info]
[networks.home.brew]
host = "brew.example.org"
port = 443
tls = true
username = 1000001
password = "secret"

[networks.tp]                      # TetraPack, reached through the brew-server
mcc = 901
mnc = 1638
via = "home"

[[cells]]
id = 1
network = "tp"
[cells.cell_info]
main_carrier = 1532
```

A radio whose home is 901/9999 that selects 901/1638 on the terminal registers on cell 1 and
appears on the server as a radio of 901/1638. `via` must name a network with a `brew` of its own,
and a network has either `brew` or `via`. `[multi_network]` is not needed for this (it is for radios
of other networks registering on this network's cells by migration, and turns migration on for
the cells while enabled). Networks and the network of
each cell can also be set on the dashboard page **Infrastructure** ("Networks" card and the
"Network" column of the location areas card); the first network added there turns `[net_info]`
and `[brew]` into the network `home`. With Multi Network on and a brew-server that offers `mni`, the
card also lists the networks the server reports (its own and the ones it links to) with "Add
network", and warns when a network reached `via` the server has an MCC/MNC the server does not
reach ("Routing with this MCC/MNC not possible").

## Radio configuration: ITSI attach or migration

How a radio registers on a cell of another network is set in the radio, per network in its list of
foreign networks, and decides which network the radio belongs to there:

| Radio setting for the other network | What the radio sends (TS 100 392-2) | Where it lands |
| --- | --- | --- |
| ITSI attach | `U-LOCATION UPDATE DEMAND` type ITSI attach / demand, with that network's MNI as address extension: the radio has an identity of its own in that network | That network: on a `via` cell the brew-server registers it there (e.g. TetraPack) |
| Migration | type migrating location updating with the radio's home MNI (clause 16.4.1.1 case b)) | Its home network: the station gives it a (V)ASSI on the air (clause 16.4.7) and names it with its home ISSI towards the brew-server, which treats it as one of its own radios (home = server's network) or registers it at its home network over a link (`mni`) |

Seen on a Motorola MTP6650 (CPS, German UI): *Mobilitäts- und Systemparameter → Netzwerke*, column
*Registrierungsmethode* per network ("ITSI-Anschluss" or "Migration"); migration also needs
*Grundfunktionen → Migration* ticked. The home network is set under *Netzwerke → Startseite* and
does not belong in the list of foreign networks. With "ITSI-Anschluss" the radio registered on a
901/1638 `via` cell as a TetraPack radio (field check 2026-11-07). Other makes have equivalent
settings; the names differ.

A migrated radio that calls or sends an SDS to a bare SSI reaches a party of the cell's network,
not of its home network: the SwMI takes the missing MNI as that of the current network (TS 100
392-2 clause 14.2.3, note 3: a migrated MS sends the MNI also for addresses of its home network).
So on a 901/1638 cell an SDS from a migrated 9999 radio to `200999` goes to 901/1638 (TetraPack);
to reach the app in 9999 the radio has to dial the full TSI. Calls and SDS from the home network
to the radio reach it on its (V)ASSI.

The same holds for groups: a migrated radio that attaches a group without an MNI is in the cell
network's group (clause 14.2.3). So the cell decides which network a radio talks in, with one
radio and one home identity ("switching the network by cell"). Field check 2026-11-07, MTP6650
with home 901/9999 and the brew-server of 901/9999 linked to TetraPack:

| Radio on | Group calls go to | The station shows |
| --- | --- | --- |
| a 901/9999 cell | the brew-server's own groups (901/9999) | a radio of this network |
| a 901/1638 `via` cell | TetraPack / Brandmeister groups (901/1638), over the brew-server's link | the radio migrated, on a (V)ASSI |

Groups bridged between the networks (talkgroup pools on the brew-server) work in both cases.

## Not yet

- Routing by the radio's *home* network on a single carrier (a migrating radio presents its home
  MNI in U-LOCATION UPDATE DEMAND). Today a cell serves one network and a radio reaches another
  network by moving to that network's cell. Home-MNI routing is the natural next step on top of
  this and would also need migration enabled on the cell and in the radios' codeplugs.
- Passing a radio's network to a core of the network's own (`brew`); with `via` and a brew-server
  that offers `mni` the server is told (see above).
- A network reached `via` with more than one cell announces only its home cell in
  `SERVICE_BTS_INFO`.
- On-air validation: this has been exercised in unit and integration tests only. The first live
  check is two carriers on different MNIs, a radio with both networks programmed, and switching
  networks on the radio while the Cells card shows which cell it registers on.
