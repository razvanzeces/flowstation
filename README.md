<div align="center">

<img src="contrib/logo/bost_flowstation_header.png" alt="Bost FlowStation" width="420"/>

### Software-defined TETRA base station (fork), built in Rust for Raspberry Pi.

[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)](https://www.rust-lang.org)
[![Branch](https://img.shields.io/badge/branch-main-00d4a8.svg)](https://github.com/Aitorrio/bost-flowstation/tree/main)

</div>

## What is Bost FlowStation?

**Bost FlowStation** is a fork of [FlowStation](https://github.com/razvanzeces/flowstation) focused on **easy Raspberry Pi installs** and **day-to-day operation from the web dashboard** — without living in SSH and `config.toml`.

Based on FlowStation by **Razvan Zeces / YO6RZV** (itself built on [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation)). Improved by **Aitor, EA4HBL**.

**Tested hardware:** LimeSDR Mini 2.0 · SXceiver · µCell BB · Ettus USRP B210 (and XC7K325T clones) · Motorola MXP600 · Motorola MTM800E · Motorola MTM5400 · Sepura STP9000 · Sepura SRG3500 · Sepura SRG3900 · Sepura SC2229

> Protocol details, Brew theory, Asterisk/DAPNET/GeoAlarm and upstream bug history live in the [original FlowStation README](https://github.com/razvanzeces/flowstation/blob/main/README.md). This page documents **what this fork adds** and **how to run it**.

---

## What’s new in this fork

| Capability | Why it matters |
|---|---|
| One-command Pi install | Boots with RF off so the dashboard is always reachable |
| First-run **Setup** wizard | Install SDR drivers and finish RF / net / Brew from the browser |
| Visual **Config** (Cell × Brew profiles) | Switch networks without hand-editing TOML |
| **LST Dispatch** console | Local group/private voice + SDS from the browser (HTTPS) |
| Access control & U-STATUS remote control in GUI | Whitelist + walkie commands (`ip` / `temp` / `info` / `restart`…) without SSH |
| **System** control panel | Restart, suspend, full power-off, **OTA**, and **panel account** from the dashboard |
| Sidebar update badge | Glance notice when a newer commit is available on the **active OTA channel** |
| Spanish-first UI (multi-language) | Ready for operators who prefer ES |
| **Multi-cell** *(new in v0.5.0)* | One station, several SDRs: each SDR is one more TETRA cell, linked for calls, SDS and handover |
| **Roaming between stations** *(new in v0.5.6)* | Separate stations on one Brew server keep a radio's group and individual calls when it changes station |
| **Multi-cell roaming** and **Infrastructure** page *(new in v0.5.6, first called Roaming / Cells)* | Calls follow the radio between the cells of a station, cells can share a location area, multi-cell stations roam with other stations; one dashboard page for cells, neighbours and reselection |
| **Group access from the Brew server** *(new in v0.5.6)* | Allow / block list and RX only per group, set on the brew-server and enforced on air: refused attachment, no floor for listeners, emergency always passes |
| **Multi Network** *(new in v0.5.6)* | One network per cell, migration (ETSI 16.4.1.1 b) and radios of other networks through the Brew server; the cell a radio uses decides the network it talks in (e.g. own Brew network or TetraPack / Brandmeister); switch on the **Infrastructure** page |
| **Ambience Listening (SS-AL)** *(new in v0.5.4)* | Dispatcher-invoked receive-only call: a supporting radio connects and keys its mic on its own (indicated on the radio, not covert) |
| **Request for Location (LIP)** *(new in v0.5.4)* | Dispatcher asks a radio for its position (once or polled); the LIP report reaches the console that requested it |

---

For current RF registration and silent-radio reachability checks, see [Radio presence](Docs/radio-presence.md).

## Installation

On **Raspberry Pi OS / Debian arm64** (always installs from branch **`main`** / channel Estable unless you override `BOST_BRANCH`):

```bash
curl -fsSL https://raw.githubusercontent.com/Aitorrio/bost-flowstation/main/contrib/install/install-bost.sh | sudo bash
```

When it finishes you should see something like:

<p align="center">
  <img src="Docs/screenshots/02-install-done.png" alt="Install finished — dashboard URL and default login" width="720"/>
</p>

| | |
|---|---|
| Dashboard | `https://<pi-ip>/` (HTTP `:80` redirects to HTTPS) |
| Default login | `admin` / `1234` |
| Config on disk | `/etc/flowstation/config.toml` (+ `.fallback` reserve) |
| Sources | `/opt/bost-flowstation` (branch `main`) |

More detail (env vars, force-clean, helper script): [`Docs/install-and-setup.md`](Docs/install-and-setup.md).

For an HTTP management client over a trusted LAN or encrypted private VPN, see the optional [authenticated HTTP access](Docs/private-http-access.md) configuration. HTTPS remains enabled; fresh installations keep the default HTTPS setup.

### µCell BB

The Setup wizard's **Install µCell** button builds [SoapyMuCell](https://github.com/Jankyneering/mu-cell-bb-drivers)
and the `mu-cell-bb_raspberrypi` overlay (not the SXceiver module). Device string: `driver=mucell`.
Gain stages match SXceiver (`dac`/`mixer`, `lna`/`pga`). After install the Pi must reboot so the
overlay loads; the wizard offers that reboot automatically.

### USRP B210 (and B200 / B210 clones)

The installer and the Setup wizard's **Install USRP (UHD)** button put in `uhd-host`, the
SoapySDR UHD module and the B2xx firmware/FPGA images. Defaults for a B2xx: transmit on
**TX/RX**, receive on **RX2**, one PGA gain stage at 40 dB (RX 0–76 dB, TX 0–89.8 dB); use a
duplexer or two antennas with enough isolation. Device string: `driver=uhd,serial=XXXXXXX`.

Many low-cost "B210" boards carry a Kintex-7 XC7K325T and refuse Ettus's FPGA image (`fx3 is in
state 5` after the image loads). They work with the open replacement image from
[kingjamez/B210_XC7K325T_Improvements](https://github.com/kingjamez/B210_XC7K325T_Improvements),
which the helper installs: `bost-setup-helper.sh install-driver uhd <serial>` pins it to the
board in `/etc/uhd/uhd.conf`, or set
`device = "driver=uhd,serial=XXXXXXX,fpga=/usr/share/uhd/images/b210_k7.bin"`. A USB 3 port or a
powered hub is required.

### Carrier accuracy (Hytera radios never find the cell)

Clone boards can transmit a kilohertz or two off frequency. Sepura radios register anyway;
**Hytera** radios do not: crossed antenna on the radio, not one access burst in the station
log, and no codeplug setting fixes it. Measure the carrier with an RTL-SDR on any nearby
computer and let the script tell you the `ppm_err` to set:

```bash
tools/carrier-offset.py 438.025        # the station's tx_freq
```

Aim for under 100 Hz (the ETSI base-station limit is 0.2 ppm). Details, calibration of the
dongle itself and what the correction does to the uplink: [`Docs/carrier-accuracy.md`](Docs/carrier-accuracy.md).

---

## First login & Setup wizard

1. Open the dashboard URL printed by the installer.
2. Sign in with `admin` / `1234` (change them later under **System → Account**).

<p align="center">
  <img src="Docs/screenshots/01-login.png" alt="Bost FlowStation login screen" width="420"/>
</p>

3. If setup is incomplete, the **Setup** wizard opens automatically (also available from the sidebar).
4. In **Select SDR**, scan hardware or install the **SXceiver** / **Lime** / **µCell** / **USRP** driver, then continue through RF, network and Brew. Enable RF and restart when ready (µCell also needs a Pi reboot so the device-tree overlay loads).

<p align="center">
  <img src="Docs/screenshots/03-setup-sdr.png" alt="Setup wizard — Select SDR" width="640"/>
</p>

The stack starts with `phy_io.backend = "None"` so the web UI works even before an SDR is ready.

---

## Configure from the web UI

Open **Config** in the sidebar. Prefer the visual forms over raw TOML — that is the point of this fork.

### Dashboard languages

Select **PT** for Portuguese (Portugal) or **FR** for French in the language picker
(inside the preferences menu on smaller screens). The choice is saved in the browser
and also used by the login page. On first access, Portuguese and French browser
language preferences are recognized automatically.

Portuguese and French translations by **Gustavo Encarnação — CS7ATL**. Protocol names,
identifiers, commands and technical acronyms are preserved. Diagnostic messages and
logs received directly from the station remain in their original form.

Translation checks: `node contrib/test-dashboard-locales.cjs`.

### TMO profiles (Cell × Brew)

1. Pick a **TMO Cell** and a **Core Net (Brew)** (or Offline).
2. **Add / Edit** opens a floating sheet with the full Cell or Brew form (including Auto RX + carrier and MHz normalization). **Save** stores the profile JSON only — it does not restart.
3. **Apply & Restart** puts the selected pair on air from the profiles on disk (edit in a sheet first if you need changes).

<p align="center">
  <img src="Docs/screenshots/08-profiles.png" alt="Config — Cell × Brew profiles" width="720"/>
</p>

Use this to jump between networks (e.g. local cell vs BrandMeister) without rewriting `config.toml` by hand.

### Live settings

Expand **Live settings** to edit the running station (RF, network, Brew, ISSI whitelist). Frequencies (TX/RX and custom duplex) are entered in **MHz** (comma or dot). On blur the UI normalizes like Motorola CPS to six decimals (e.g. `432,2` → `432.200000`); the running `config.toml` still stores **Hz**.

**Apply & Restart** writes those forms into `/etc/flowstation/config.toml` only — it does **not** update a Cell/Brew profile JSON.

Under **Hardware RF** (collapsed by default): the SDR **device** string is read-only (set in Setup); PPM, antennas and gain stages follow that driver. Enter numbers with comma or dot; empty gain/antenna fields omit the keys so the device defaults apply. Saving rejects gain stages that do not belong to the selected family (e.g. Lime `pad` on SXceiver).

<p align="center">
  <img src="Docs/screenshots/09-live-settings.png" alt="Config — live settings, RF frequencies and network identity" width="720"/>
</p>

### Access control (ISSI whitelist)

The ISSI whitelist lives inside the **Cell** form (profile sheet) and inside **Live settings** — not as a separate page section. Empty list = open network; with entries, only listed radios may register.

- In a **profile sheet**, **Save** stores the list on that Cell profile.
- In **Live settings**, **Apply & Restart** writes it to the active config only.
- **Apply & Restart** on Profiles puts the selected Cell on air, including its saved whitelist.

U-STATUS remote control stays station-wide (not stored in Cell/Brew).

### Authentication and air-interface encryption (Security page)

The **Security** page (SYSTEM → SECURITY, or click the SECURITY row in the sidebar) implements
the TETRA security procedures of ETSI EN 300 392-7 with the algorithms ETSI published in 2024,
every one checked against independent test vectors:

- **Authentication (TAA1):** off / optional / required, mutual authentication, and the subscriber
  key table — one 128-bit K per radio, identical to the key programmed into it. Radios that pass
  show an **AUTH** badge in the Radios table. Authentication is an access-control handshake and is
  fine on amateur allocations.
- **Encryption (class 2):** the static cipher key (SCK), its number and version, the algorithm
  (**TEA1**, flagged weak — it keeps only 32 of its 80 key bits; **TEA2**; **TEA3**) and the
  clear talkgroups for radios without encryption (this fork's mixed-cell policy). Radios seen
  encrypting show a lock badge with the key they use.
- **Over-the-air rekeying (OTAR):** **Send SCK** next to a radio's key delivers the SCK sealed
  under that radio's K (clause 4.5.2); the radio's answer appears under the key. Leave
  **Encrypt the cell** off while you distribute the key — the SCK is then only used for OTAR and
  the cell stays in clear — and switch it on once every radio reports the key as accepted.

The same settings live under `[security]` in `config.toml` (see the example config). Air-interface
*encryption* is not permitted under amateur licences — enable it only on a licensed private
network.

<p align="center">
  <img src="Docs/screenshots/10-access-control.png" alt="Config — ISSI whitelist / access control" width="720"/>
</p>

### Remote control (U-STATUS)

In **Control remoto (U-STATUS)**, authorize radios and map status codes to actions (`ip`, `temp`, `info`, `restart`, `shutdown`, `kick_all`). Configured from the dashboard; not stored inside Cell/Brew profiles.

<p align="center">
  <img src="Docs/screenshots/11-remote-control.png" alt="Config — remote control via U-STATUS" width="720"/>
</p>

### Advanced (raw `config.toml`)

Collapsed under **Advanced** for power users: red warning, then **Save** and **Apply & Restart**. Full annotated reference: [`example_config/config.toml`](example_config/config.toml).

<p align="center">
  <img src="Docs/screenshots/12-raw-config.png" alt="Config — raw config.toml editor with Save and Restart" width="720"/>
</p>

---

## Multi-cell: one station, several SDRs *(v0.5.0)*

Every extra SDR can run one more TETRA cell from the same station — for example a Pi with two Pluto+ on the network. All cells share one network identity (MCC/MNC), one dashboard and one set of network links, and radios move between them like between sites of one system.

> **Status:** new in **v0.5.0** — complete in code and unit tests, **not yet validated on air**. Please report how it behaves with your radios.

### Setting it up

- **Dashboard:** the **Cells** card on the **Infrastructure** page lists every cell (RF state, carriers, SDR, radios). *Scan* finds your SDRs; pick the device, a free carrier and optionally a colour code and location area (LA; empty = the primary's), and *Add cell* — the station restarts. *Remove* takes a cell out again.
- **By hand:** add a `[[cells]]` entry per extra SDR. It inherits everything from `[cell_info]` except what you override:

  ```toml
  [phy_io.soapysdr]
  device = "driver=plutosdr,uri=ip:192.168.2.1"   # the primary cell's SDR (required once there are cells)

  [[cells]]
  id = 1                                          # 1-7

  [cells.cell_info]
  main_carrier = 1525                             # unique per cell
  colour_code = 2

  [cells.soapysdr]
  device = "driver=plutosdr,uri=ip:192.168.3.1"
  tx_freq = 438125000
  rx_freq = 433125000
  ```

  Rules: up to 8 cells (ids 1-7 plus the primary), every carrier unique, same `freq_band` and `custom_duplex_spacing`, and every cell (the primary too) names its own SDR `device`. See the commented example at the end of [`example_config/config.toml`](example_config/config.toml).

### What linking gives you

Cells are **linked** when Brew or LST Dispatch is enabled; without either, each cell runs on its own.

- **Group calls** — from the network or from any radio — reach every cell with members of the group. One talker per group across all cells; an emergency call takes the floor from a normal one.
- **Individual calls and SDS** work between radios on different cells; group SDS reaches members on every cell. Only traffic for nobody on site goes out to Brew, and talkgroups in `local_ssi_ranges` link the cells without ever reaching Brew.
- **Asterisk (SIP)** and **WX/METAR** work from every cell; dashboard WX changes apply to all of them.
- Voice passed between cells is buffered and played out on the receiving cell's own timing (separate SDRs have separate clocks).

### Moving between cells

- Every cell automatically advertises the others as neighbours (no need to list them in `neighbor_cells_ca`), with what radios need to find and rank them, and supporting both unannounced and announced reselection.
- **One location area or one per cell:** cells inherit the primary's `location_area`. Within one location area radios change cell without registering (ETSI 18.3.4.7.1a), so group calls run on every cell of the area where a member is registered, a call from the network to a radio is set up on all of the area's cells (the cell where it answers keeps it), and a presence check is sent on all of them; the radio registers where it answers. With an own `location_area` per cell radios register on every change, so station and Brew server always know the exact cell, and group calls stay on the members' cells.
- **Unannounced:** a radio that registers on another cell is dropped silently on the old one, and restores its group call on the new one.
- **Announced (D-NEW-CELL):** the new cell joins the radio's group calls before it arrives; a registration the radio forwards in U-PREPARE is processed by the new cell and answered in D-NEW-CELL.
- Reselection thresholds and hysteresis are configurable in `[cell_info.cell_reselect]` (defaults 20/10/10/6 dB), the minimum access level in `[cell_info] rxlev_access_min_dbm` (default −110 dBm). How they work together: [`Docs/cell-reselection.md`](Docs/cell-reselection.md).

### Dashboard and telemetry

The **Infrastructure** page (left menu) edits everything about moving between cells in one place: the Cells card (add or remove a cell, moved here from Home), location area per cell, reselection thresholds and hysteresis, minimum access level, migration, presence check, and each cell's own neighbour list (`[[cells.cell_info.neighbor_cells_ca]]` for extra cells; without one a cell advertises only the station's cells). It also shows each cell's global id and which roaming extensions the Brew server offers. With a brew-server that offers `cell-directory` it lists the other stations' cells (online or offline), names each neighbour entry after the server cell it matches, warns when that cell does not list this one back or broadcasts other levels or services than the entry says, and fills a new neighbour in from the list.


The registered-radios table shows every cell's radios with a C0/C1… badge, the Calls page includes calls on every cell, and telemetry carries each radio's cell (`MsCell` event).

### Known limits

- The bit layout of the reselection parameters and the neighbour carrier extension follows our reading of EN 300 392-2; check a radio's field-test display if reselection misbehaves.
- A group call spanning cells shows once per cell in the Calls list (each cell runs its own traffic channel).
- Individual calls with a Brew party (e.g. the TetraLink app) follow the radio to another cell (U-CALL RESTORE, answered through the site switch; built on David Harcus' individual call handover, PR #30). Calls between two radios of the station survive a change of cell too, announced or not: the cell holding both radios hands the call over through the site switch, and a radio coming to its peer's cell is joined into a local call there (no server extension needed).
- Within one location area an individual SDS, or a call from another radio of the station, to an idle radio goes to the cell it last registered on, until its next transmission or presence check moves it.
- CPU and USB/network bandwidth for several SDRs on one Pi are untested; start with two cells.

Design notes and per-phase status: [`Docs/multi-cell-plan.md`](Docs/multi-cell-plan.md).

### Several networks in one station

Cells can serve different TETRA networks: a `[networks.<name>]` table gives each network its MCC/MNC and its own Brew core (or none, for a network switched inside the station), and `network = "<name>"` binds the primary cell at the root and each `[[cells]]` entry. A radio user picks the network on the terminal and lands on that network's cells and core. See [`Docs/networks.md`](Docs/networks.md).

A network can also be reached *through* the Brew server of another of the station's networks, with no credentials of its own on the station: `via = "<network>"` in `[networks.<name>]` (Multi Network "Infrastructure"; the server must offer the `mni` extension and reach that network, e.g. over its TetraPack link). Networks and the network of each cell can be edited on the **Infrastructure** page. See [`Docs/networks.md`](Docs/networks.md).

## Roaming between stations over Brew *(v0.5.6)*

Two or more separate stations on the same Brew server can keep a radio's calls when it moves from one to the other. The radio decides when it changes cells (TETRA cell reselection, ETSI TS 100 392-2 clause 18.3.4); the stations and the server only make sure the call is there when it arrives.

> **Status:** field-tested with two single-cell stations (Pluto+ and SX1255) and a Motorola MTP6650, October/November 2026. **Not yet validated on air:** a multi-cell station roaming with another station, and calls between two radios across stations (`call-handoff`).

### What you get

- **Group calls** go on after a cell change: a listener hears the rest of the over on the new station (gap about 0.5 to 1.5 s), a talker holding PTT keeps the floor there.
- **Individual calls** (simplex and duplex) with a Brew party are restored on the new station (U-CALL RESTORE). A call the radio gave up (link lost at the cell edge) ends for the other party at once, so they can call back.
- **Calls between two radios** survive a station change both ways (extension `call-handoff`, single-cell stations): when both radios are on one station and one leaves, the station hands the call to the network for the new station; when they are on two stations and one comes to the other's, that station joins both legs into a local call again. The radio that stays notices nothing.
- **The old station lets the radio go**: no stale registration, no traffic channel left open for a group nobody listens to any more.
- **Announced changes** (U-PREPARE naming the other station's cell) are prepared on the target through the server: it registers the radio (forward registration) and joins its group call before the radio arrives. Unannounced changes and type 3 announcements work without that.

### What it needs

- [brew-server](https://github.com/ysamouhos/brew-server) 1.20.0 or newer (when released), or another Brew server that offers the extensions (`X-Brew-Extensions` on the WebSocket upgrade; see brew-server `docs/Handover.md` and `docs/Brew-Extensions.md`). The station uses them only when offered; with TetraPack, Brandmeister or an older server nothing changes.
- Single-cell and multi-cell stations both take part. On a multi-cell station the site switch hands each request from the server to the cell of the target carrier and each answer back to the cell that asked. Between stations a multi-cell station offers type 2 only (no type 1 with a channel of the other station); between its own cells nothing changes.
- Each station lists the other as a neighbour (dashboard page **Infrastructure**, or by hand). That page also shows whether the server offers the extensions, and with `cell-directory` lets you pick the other station's cell from the server's list and checks that both sides list each other. Example for a station on carrier 1528 whose neighbour is on 1530 (and the reverse entry on the other station):

  ```toml
  [cell_info]
  rxlev_access_min_dbm = -100            # same on both stations

  [cell_info.cell_reselect]               # values of the field test (defaults 10 / 20)
  fast_threshold_db = 30
  slow_threshold_db = 10                 # added to the fast threshold (clause 18.5.4)

  [[cell_info.neighbor_cells_ca]]
  cell_identifier_ca = 1
  cell_reselection_types_supported = 2   # forward registration, no expedited reselection (table 18.22)
  neighbor_cell_synchronized = false
  cell_load_ca = 0
  main_carrier_number = 1530
  location_area = 1                      # the neighbour's real location area

  [cell_info.neighbor_cells_ca.bs_service_details]
  registration = true                    # every field left out is broadcast as false
  deregistration = true
  system_wide_services = true
  voice_service = true
  ```

  Without `bs_service_details` the neighbour is broadcast without registration and voice, and radios will not use it. How the thresholds work: [`Docs/cell-reselection.md`](Docs/cell-reselection.md).

### Who may use a group (`group-access`)

With a brew-server that offers the `group-access` extension the station gets, live and again after every reconnect, which radios may use which group: an allow list (only these ISSIs) or a block list (everyone but these), and "RX only" (every radio, or only the listed ones, listens but may not talk). The station enforces it on air:

- A radio the group does not admit cannot attach: the registration or attach acknowledgement lists the group as refused with "Temporary 1 detachment" (ETSI TS 100 392-2 clause 16.8.2, table 16.52), so the radio may try again after the rules change.
- An RX-only radio joins running calls to listen; its D-CONNECT says "not allowed to request for transmission" (clause 14.8.43) and a PTT is answered "transmission not granted" (clause 14.5.2.2.1 b)). It cannot start a call on the group (D-RELEASE, cause "Requested service not available"; ETSI does not name a cause for this case).
- Emergency calls always pass. A group without rules, or a server without the extension, stays open as before.

The lists are edited on the server (dashboard Directory page); the rules apply to all cells of the station.

### Known limits

- Most radios change cells unannounced (a listener always, clause 18.3.4.7.1), so a short break stays. The MTP6650 never announced with a target cell in the field test (its background scan of the neighbour needs synchronized cells), so announced types 1 and 2 are covered by tests only.
- A radio that loses the link at the cell edge before it reaches the new station gives up an individual call; that is the radio's decision (clause 16.4 a)).

The server side and the Brew messages are documented in brew-server: `docs/Handover.md` (operators) and `docs/Brew-Extensions.md` (protocol for other server and Basestation implementations).

## Multi Network: radios of other networks

> **Status:** tested on air 2026-11-07 (Motorola MTP6650, a 901/1638 `via` cell next to a 901/9999 cell, brew-server linked to TetraPack): a radio of the server's own network migrating onto the `via` cell, the same radio registered as a TetraPack radio (ITSI attach) and seen at the server as a visitor, group calls in TetraPack / Brandmeister groups, and duplex calls and SDS across the networks. Also on air: a radio whose home is a third network linked to the server (901/1234) migrating onto the station, and the refusal of a home network the server does not have up.

Switching networks by cell: one radio with one home identity (e.g. 901/9999) talks in this server's network on a 9999 cell and in TetraPack / Brandmeister groups on a 901/1638 `via` cell, where it migrates; bridged groups work on both. Table and radio settings in [`Docs/networks.md`](Docs/networks.md).

A radio whose home network (MCC/MNC) differs from the cell's can register here by migration (ETSI TS 100 392-2 clause 16.4.1.1 case b)). Turn it on with the switch next to the "Networks" heading on the Infrastructure page, or a `[multi_network]` section (see `example_config/config.toml`); while it is enabled the cells announce migration in SYSINFO by themselves (clause 18.5.2.1), whatever `migration` in `[cell_info]` says. The station gives the radio a (V)ASSI and its home groups a (V)GSSI (clauses 16.4.7, 16.8.7); groups of this network keep their GSSI. With a brew-server that offers the `mni` extension the visitor shows up at the server under its home ISSI, and the server takes its registration, group calls, individual calls and SDS to its home network (brew-server `docs/Multi-Network.md`); only networks the server reaches and has up are accepted. With any other server visitors work only inside this station. A radio of this network coming back from another one (case c)) registers as usual. Not yet: following a visitor to another station or cell (only the station that gave the (V)ASSI knows it; the others reject it with "ITSI/ATSI unknown" so that it registers again), temporarily disabled radios (case d)), authentication of visitors.

Whether a radio migrates or attaches with an identity of its own in the other network is set in the radio, per network: with "ITSI attach" it registers as a radio of that network (on a `via` cell the brew-server sends it there, e.g. to TetraPack); with "Migration" it keeps its home network and gets a (V)ASSI here. On a Motorola MTP6650 this is *Netzwerke → Registrierungsmethode* per network plus *Grundfunktionen → Migration*; details in [`Docs/networks.md`](Docs/networks.md) ("Radio configuration").

## Stability and system robustness

This fork hardens day-to-day operation so your BTS keeps working — and stays recoverable — in most of the situations that used to mean “reinstall from scratch”.

Robustness around system config files has been improved so the instance can keep running even when things go badly wrong, then get you back to a healthy station from the web UI.

**What protects you in normal use**

- Dashboard writes **validate before they touch disk**, so ordinary Config / Save live / Apply flows should not brick the service.
- Prefer the visual forms over SSH hand-edits of `config.toml`.
- **No SDR / RF off** (`phy_io.backend = "None"` or Soapy failing to open) still starts the dashboard — same path as first-boot Setup — so you can finish drivers and RF from the browser.

**If you do break the live config** (typical after a bad SSH edit)

- Install keeps a sibling reserve: **`/etc/flowstation/config.toml.fallback`** (known-good; **not** overwritten when you save from the GUI).
- If the primary fails to **parse** or **validate** at boot, the service loads `.fallback`, brings the dashboard up, and shows a red warning banner.
- From there you can recover **without reinstalling**: open **Config**, **Apply & Restart** a Cell × Brew profile you already saved — that rewrites a valid primary `config.toml` even if SSH left it unusable — then restart once more if needed.
- Or fix the primary in the visual forms / raw editor / **Restore `.bak`**, then Restart.

Refresh the reserve when you have a config you trust:

```bash
sudo cp /etc/flowstation/config.toml /etc/flowstation/config.toml.fallback
```

**Re-running the installer**

If you ever need to run [`contrib/install/install-bost.sh`](contrib/install/install-bost.sh) again: it is **repair-oriented**. It keeps an existing `/etc/flowstation/config.toml` (and may refresh `ota_channel` to match the install branch), creates `.fallback` only when missing (never overwrites your reserve), aligns `/opt/bost-flowstation` to `origin/main` with `reset --hard` (keeps `target/`), and refreshes the service/helper pieces without wiping your station identity. For a full source re-clone keeping config: `sudo env BOST_FORCE_CLEAN=1 bash` with the same curl one-liner.

---

## Day-to-day dashboard

<p align="center">
  <img src="Docs/screenshots/04-sidebar.png" alt="Dashboard sidebar — monitor pages and live BS/Brew status" width="280"/>
</p>

| Page | Use it for |
|---|---|
| **Radios** | Registered terminals, RSSI, kick / SDS, timeslot view |
| **DGNA** | Assign / deassign talkgroups over the air |
| **Calls / Last Heard / Log** | Live traffic and diagnostics |
| **RF / Health** | Spectrum / constellation and subsystem health |
| **Config** | TMO profiles (sheets), live settings, Cell ISSI whitelist, remote U-STATUS, advanced TOML |
| **System** | Host metrics, service control, OTA, panel account, station backup (`.bptbs`) |
| **Setup** | Re-run first-boot helper anytime |

---

## System control & OTA updates

Restart, suspend, full power-off and OTA live in the **System** hero (top-right actions) — not in Config.

<p align="center">
  <img src="Docs/screenshots/05-system-control.png" alt="System — update banner and service control buttons" width="720"/>
</p>

- **Reiniciar** — soft restart of `bluestation-bs`
- **Suspender** — soft standby: radio stack stops, dashboard stays up; the button becomes **Iniciar** to bring the station back
- **Apagar** — full host shutdown (`systemctl poweroff`); you will likely need to cycle power to boot again
- **Canal OTA** — **Estable** (`bost`, day-to-day) or **Beta** (`beta`, previews); persisted as `[dashboard] ota_channel`
- **Actualizar** — OTA on the selected channel: `git fetch` + `reset --hard` to that branch (keeps `target/` for incremental builds), rebuild only if the running binary is behind, install, restart

When a newer commit is on GitHub for the **active channel**, a banner appears above the System hero and a matching badge shows in the sidebar (click → System).

### Station backup (`.bptbs`) and profile packs (`.ptbs`)

- **System → Backup** — export/import a full station file (`.bptbs`): live `config.toml`, Cell/Brew profiles, setup/fallback siblings, and saved Wi-Fi passwords (NetworkManager). Import replaces the destination station, **keeps** local `[dashboard] ota_channel`, scrapes bad `source_dir` paths, merges Wi-Fi by SSID (does not delete networks absent from the backup), then restarts.
- **Config → TMO profiles** — export/import Cell/Brew profiles only (`.ptbs`). Does **not** change live config or restart; use **Apply & Restart** when you want them on air.

Treat `.bptbs` as sensitive (Wi-Fi PSKs + dashboard credentials inside config).

The OTA dialog is a three-step flow (channel → what's new → progress). Expand **Ver todo** only if you need the full build log:

<p align="center">
  <img src="Docs/screenshots/06-ota-updating.png" alt="OTA update in progress with progress bar" width="520"/>
</p>

<p align="center">
  <img src="Docs/screenshots/07-ota-complete.png" alt="OTA update completed — restarting" width="520"/>
</p>

Compiles on a Pi can take several minutes — leave the window open until it finishes.

### Group call timers (Hangtime / Call timeout)

Under **Config → Advanced network / timers**, **Call timeout** is the maximum length of one group call (ETSI T310-style), not each PTT. Hangtime keeps the same call open between quick turn-taking (including LST / Brew), so a long QSO can hit the default ~120 s ceiling and radios may show PTT denied — raise the value or set **0** (unlimited). Field help (**?**) explains each timer; see [Docs/config-timers.md](Docs/config-timers.md).

### Panel account (login)

Under **System → Account** you manage the single dashboard login (the same `[dashboard]` username/password in `config.toml`) without editing TOML by hand. Station-wide — like U-STATUS, **not** part of Cell/Brew profiles.

<p align="center">
  <img src="Docs/screenshots/13-dashboard-user.png" alt="System — panel account: change dashboard username and password" width="720"/>
</p>

- Change **username** and/or **password** (current password required).
- If the panel was open (no auth), **enable login** from the same card.
- Changes apply immediately and persist across restart; you are asked to sign in again.

Default after install remains `admin` / `1234` — change it here on first opportunity.

---

## Optional: build from source

Only if you are not using the installer:

```bash
git clone https://github.com/Aitorrio/bost-flowstation.git -b bost
cd bost-flowstation
cp example_config/config.toml ./config.toml
# Prefer phy_io.backend = "None" until RF is configured via Setup / Config
cargo build --release -p bluestation-bs
./target/release/bluestation-bs config.toml
```

---

## Branches

| Branch | Purpose |
|---|---|
| **`main`** | This fork — install script, Setup, visual config, OTA, Spanish UI |
| `beta` | Upstream active development |

---

## Upstream & community

- **This fork:** [github.com/Aitorrio/bost-flowstation](https://github.com/Aitorrio/bost-flowstation) (branch `main`)
- **Community:** [Telegram](https://t.me/+CKsriSWVp_oxNTg0)

For Asterisk, DAPNET, Snom, GeoAlarm and deep protocol notes, use the upstream docs — this fork inherits those features but documents the **Pi + dashboard** path here.

---

## Credits

- **Razvan Zeces / YO6RZV** and the FlowStation community for the base station stack
- **Mihajlo YU4MSH**, **Torben DJ2TH**, **Joaquin EA5GVK** and others credited upstream
- **MidnightBlueLabs** — [tetra-bluestation](https://github.com/MidnightBlueLabs/tetra-bluestation)
- **Aitor, EA4HBL** — this fork (install, Setup, visual config, OTA UX, Spanish UI).
- **Yiannis Samouhos, ysamouhos** — Multi-cell support with multiple SDR and AIE.
- **David Harcus, MM7FDM** — TETRA authentication (TAA1), TEA2/TEA3, SCK OTAR, the Security dashboard page, USRP B210 support, the WAP packet-data (SNDCP) gateway with its live status page, end-to-end encryption (E2EE) of SDS, and individual call handover between the cells of a multi-cell station (PR #30).
- **Sebastian, OE4SXM** ([sebastianmadl](https://github.com/sebastianmadl)) — roaming between stations over Brew (group and individual calls across stations, announced and unannounced cell reselection), multi-cell roaming (individual calls with the network follow the radio between cells, built on David's PR #30; cells sharing a location area; multi-cell stations roaming with other stations), the Infrastructure dashboard page (formerly Roaming / Cells) with the Brew server's cell list, configurable minimum access level and migration flag, group access and RX only from the Brew server, English changelog.

---

## License

Apache 2.0 — see [LICENSE](LICENSE)
