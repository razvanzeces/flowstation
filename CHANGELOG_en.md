# Changelog — Bost FlowStation → PTBS (English)

Operator notes. English version of [`CHANGELOG.md`](CHANGELOG.md) (Spanish), which the dashboard OTA reads and shows the sections after your current version from. Both files carry the same entries.

## v0.5.6 — Roaming between stations (Brew) and between cells

- **Includes four upstream `beta` pull requests**: by David Harcus, MM7FDM, #37 (WAP packet data channel fix), #38 (several networks in one station, the base of Multi Network) and #39 (carrier accuracy tool); by dn9cho, #40 (APRS weather stations by SDS). Their entries are under "Not yet released" below.
- **A network with its own server from the dashboard.** On the Infrastructure page's "Networks" card, "Reached" now also offers "own server": host, port, TLS, login and password of that network's Brew server (`[networks.<name>.brew]`, from PR #38). Before, it could only be added in the config.
- **Roaming between stations.** Stations on the same Brew server keep a radio's calls when it moves from one to another, by announced or unannounced cell reselection (ETSI TS 100 392-2 clause 18.3.4). Group calls: a listener keeps hearing the over on the new station, a talker holding PTT keeps the floor. Simplex and duplex individual calls with a Brew party are restored on the new station (`U-CALL RESTORE`); a call the radio gives up ends at once for the other party. The old station lets the radio go without leaving registrations or channels behind. Announced reselection with a target cell prepares the target station through the server (types 1/2), type 3 works without it. Needs [brew-server](https://github.com/ysamouhos/brew-server) 1.20.0 or newer (when released) with the extensions `bts-info`, `handover-v1`, `ms-cell`, `ms-moved`, `ms-reselection`, `call-restore`; other servers (TetraPack, Brandmeister) work as before. Tested on air with two stations and a Motorola MTP6650. See "Roaming between stations over Brew" in the README.
- **Multi-cell: calls follow the radio between the cells of a station — provisional, not yet validated on air.** Simplex and duplex individual calls with a Brew party (e.g. the TetraLink app) continue when the radio changes cell mid-call: the new cell answers the radio's `U-CALL RESTORE` with `D-CALL RESTORE` (clause 14.5.1.2.4). A call the radio gives up, or that is not restored within 8 s, ends at once for the other party. Builds on the individual call handover by David Harcus, MM7FDM (PR #30).
- **Multi-cell: cells can share a location area — provisional, not yet validated on air.** Within its location area a radio changes cell without registering (clause 18.3.4.7.1a). Group calls run on every cell of the area, presence checks are sent on all of them so the radio registers where it is, and an individual call from the network is offered on all cells of the area; the cell where the radio answers takes it.
- **Multi-cell stations roam with other stations — provisional, not yet validated on air.** A station with several cells takes part in roaming between stations like a single-cell one: forwarded registrations, announced reselection and restored calls are handled on the right cell. Between stations it offers type 2 only.
- **New dashboard page "Infrastructure".** (Named "Roaming / Cells" in earlier builds; menu icon: a radio tower linked to its cells and networks.) One page for the cells of this station and everything that decides when and how radios move: the Cells card (add and remove cells, SDR scan, optional location area for a new cell; moved here from Home), global cell id (`<Brew login>/<cell>`) and location area of each cell, the neighbour list of each cell (extra cells may now have their own, `[[cells.cell_info.neighbor_cells_ca]]`), minimum access level, migration, presence check, and the reselection values under "Cell reselection (advanced)". Every field has a "?" help with the ETSI clause. Saving checks the config, writes `config.toml` (the previous file is kept as `config.toml.cells.bak`) and restarts the station.
- **Cells of the Brew server on the Infrastructure page.** With a brew-server that offers `cell-directory` the page lists the cells of the other stations (online or offline, carrier and downlink frequency, LA, access level, MS power, their neighbours), warns when a neighbour entry does not match what that cell broadcasts or when it does not list this cell back, and "Add as neighbour" fills in an entry from the list.
- **Configurable minimum access level.** `[cell_info] rxlev_access_min_dbm` (−125 to −50 dBm in 5 dB steps, default −110), sent as RXLEV_ACCESS_MIN in SYSINFO (table 21.65); extra cells may override it. Also in Config → Cell/RF → Advanced RF, next to a "Migration supported" checkbox for `[cell_info] migration` (sets the SYSINFO bit; accepting radios of other networks also needs `[multi_network]`, see below). Guide: `Docs/cell-reselection.md`.
- **Calls between two radios survive a station change — provisional, not yet validated on air.** A simplex or duplex call between two radios of one station goes on when one radio moves to another station and restores it there: the station hands the call to the network (the leg that stays keeps its channel, the radio that stays notices nothing; a talker that left loses the floor at the break, clause 14.5.1.2.4). A duplex call whose leg goes silent is held 8 s for that. The other way round, a radio that comes to its peer's station is joined into a local call there again. Needs brew-server with the `call-handoff` extension; between stations, single-cell stations only for now.
- **Multi-cell: calls between two radios survive a change of cell — provisional, not yet validated on air.** A simplex or duplex call between two radios of the station goes on when one of them changes cell and restores it (`U-CALL RESTORE`, clause 14.5.1.2.4), announced or not: with both radios on one cell, that cell hands the call to the new cell through the site switch; with the radios on two cells, the cell a radio comes to joins both legs into a local call again. The radio that stays notices nothing, a talker that left loses the floor at the break, and a leg that goes silent is held 8 s for the restore. Within the station, no server extension needed. Completes the open points of the individual call handover of PR #30 (David Harcus, MM7FDM) for two radios of the station; an announced change to the peer's own cell no longer pre-provisions a second leg there.
- **Who may use a group, from the Brew server.** With a brew-server that offers `group-access` the station gets each group's lists live (allow only these ISSIs or block these, and "RX only" for every radio or for some) and enforces them itself: a radio the group does not admit cannot attach (`D-LOCATION UPDATE ACCEPT` / `D-ATTACH/DETACH GROUP IDENTITY ACK` list it as refused, "Temporary 1 detachment", clause 16.8.2, table 16.52), an RX-only radio joins the call without the right to ask for the floor (clause 14.8.43) and its PTT gets "transmission not granted" (clause 14.5.2.2.1 b)); a radio that may not talk cannot start a call on the group either (`D-RELEASE`, cause "Requested service not available"). Emergency calls always pass. Changes on the server's dashboard arrive without a restart; with other servers nothing changes.
- **Server features on the Infrastructure page.** The Brew extensions the server offers show in a collapsible "Server features" list, grouped (Roaming, Calls, Cells, Groups), as pills with a green (offered) or red (not offered) dot, now including `call-handoff`, `group-access`, `issi-check` and `mni` (Multi Network: the server checks whether an ISSI is in use in the other network before registering it there, and takes radios of other networks on stations with `mni`).
- **Migration: radios of other networks on this station (Multi Network, step 6) — tested on air 2026-11-07** (a radio of the Brew server's network, and a radio of a third network linked to the server, 901/1234; a network the server does not have up is refused). With `[multi_network] enabled = true` and the home network (MCC/MNC) listed in `networks`, a radio of another network registers here by migration (ETSI TS 100 392-2 clause 16.4.1.1 case b)): the station answers the first `U-LOCATION UPDATE DEMAND` on the USSI with `D-LOCATION UPDATE PROCEEDING` and a (V)ASSI, and the radio registers on it (clause 16.4.7). Its home groups get a (V)GSSI (clause 16.8.7); groups of this network keep their GSSI. A radio of this network coming back from another one (case c)) registers as usual. With a brew-server that offers `mni` the visitor reaches its home network through the server (see below); with other servers it stays inside the station. `[multi_network]` on turns migration on by itself (BS service details in SYSINFO, clause 18.5.2.1), so `migration = true` in `[cell_info]` is no longer needed for it; the dashboard shows the checkbox ticked and greyed out. Whether a radio migrates or attaches with its own identity in the other network is a radio setting per network (ITSI attach or migration); `Docs/networks.md` explains both, with the Motorola CPS fields. The Brew server's group access rules check a migrated radio of the server's own network under its home ISSI, not its (V)ASSI (the field check refused its group). An individual call or SDS from the server for such a radio reaches it on its (V)ASSI. A group call of the server's network reaches such a cell only on the (V)GSSI of a migrated radio; anything else is dropped, so a group of the server's network is never played out as a group of the cell's network (the field check heard an app's call on a TetraPack cell). Migration from the server's own network onto a `via` cell works on air (Motorola MTP6650, 2026-11-07). So the cell decides the network a radio talks in ("switching networks by cell"): with home 901/9999, on a 9999 cell it is the Brew server's radio, on a 901/1638 cell it migrates and its groups are TetraPack / Brandmeister groups (table in `Docs/networks.md`). Note: a migrated radio that dials only an SSI calls a party of the cell's network (TS 100 392-2 clause 14.2.3: the SwMI takes the missing MNI as that of the current network); to reach its home network it has to send the full TSI. A switch next to the "Networks" heading on the Infrastructure page turns `[multi_network]` on and off (saves and restarts). The card lists the networks the Brew server reports (`SERVICE_NETWORKS`: its own network and the ones it links to, up or down) with "Add network", which takes their MCC/MNC so every station uses the same values, and marks each network: routable, link down, or "Routing with this MCC/MNC not possible" when the server does not reach it (also when the primary network's MCC/MNC is not the server's own). Without the section nothing changes: "Migration not supported", now answered on the USSI as the clause asks.
- **Multi Network with the Brew server (step 7) — tested on air 2026-11-07** (a TetraPack radio on a `via` cell as a visitor: group calls, duplex calls and SDS across the networks). With a brew-server that offers `mni` the station names it in `SERVICE_BTS_INFO` and shows each visitor with its home identity: `SERVICE_MS_NETWORK` before its registration, its home ISSI instead of the (V)ASSI and the home GSSI instead of the (V)GSSI (`SERVICE_GROUP_NETWORK` for groups of this network, `SERVICE_CALL_NETWORK` before a group call of its home network, and with the calling network for individual calls and SDS). The other way round, a group call of the home network comes in on the (V)GSSI and a call or SDS to the visitor on its (V)ASSI. Migration is accepted only from networks the server reaches and has up (`SERVICE_NETWORKS`), and also listed in `networks` if that is set. (V)ASSIs and (V)GSSIs never go to the network.
- **Multi Network: a radio of another network that does not migrate is rejected — provisional, not yet validated on air.** With `[multi_network]` on, a location update that names another network (address extension) but is not a migrating one, e.g. a radio answering a `D-LOCATION UPDATE COMMAND` after its home network was changed, is no longer registered under its bare ISSI (that is a radio of this network) but rejected with "ITSI/ATSI unknown" (ETSI TS 100 392-2 clauses 16.4.1.1 b), 16.4.7.3), so the radio registers again as a migrating one. Without the section nothing changes.
- **Faster late entry after a station change.** The station repeats the group call's D-SETUP as soon as an arriving radio attaches its groups: about 1 s gap instead of 5 s.
- **AIE fix: signalling on the traffic channel (FACCH/STCH) is encrypted.** With air interface encryption (TEA) the messages the station steals from a voice channel (e.g. D-TX GRANTED / D-TX CEASED) went out in clear to radios registered encrypted. The MAC-RESOURCE is now kept as it is until the actual transmit slot and encrypted there with that slot's key stream: ESI instead of the ISSI, KSS(0 to 123) in the first half slot and KSS(216 to 339) for a MAC-END in the second (ETSI EN 300 392-7 clauses 6.4.1 table 6.4 and 6.7.1.2). If it cannot be encrypted it is not sent in clear. Thanks to [lx-dxb](https://github.com/lx-dxb) ([BOST-FlowStation-FACCH-STCH-Air-Interface-Encryption-Fix](https://github.com/lx-dxb/BOST-FlowStation-FACCH-STCH-Air-Interface-Encryption-Fix)).
- **OTAR fix: "Send SCK" from the dashboard reaches the radio.** The dashboard sent every command to CMCE, which dropped the SCK OTAR ("ignoring unsupported control command OtarSck"), so the key never went out on air. The dashboard now has its own channel to MM: SCK OTAR and DGNA go straight to MM, and the dashboard and remote control share one command → entity mapping. Thanks to [lx-dxb](https://github.com/lx-dxb) ([bost-flowstation-otar-fixed](https://github.com/lx-dxb/bost-flowstation-otar-fixed)).
- **No more "-modified" after installing.** The installer no longer changes a file in the checkout, so builds no longer show `-modified`, and local changes no longer block an update.

## v0.5.5 — µCell wizard (SoapyMuCell)

- **µCell wizard.** The wizard and the Setup page can install the `mucell` driver (µCell BB board, not the same as SXceiver): clones/builds SoapyMuCell and the `mu-cell-bb_raspberrypi` overlay, writes `device = "driver=mucell"`, and if the helper prints `NEED_REBOOT` offers to reboot the Pi (`POST /api/system/reboot`) to load the device tree. Gain/antenna families aligned with SXceiver; validation `SoapyDriverFamily::MuCell`.

## Not yet released — Authentication, TEA2/TEA3, OTAR, SDS E2EE, WAP and USRP B210

- **APRS weather stations by SDS (`APRSWX`, PR #40, dn9cho).** The WX service takes a third command: an SDS `APRSWX <CALL>` (e.g. `APRSWX DN9CHO-13`) to the service ISSI returns that APRS/CWOP weather station's latest report from aprs.fi (temperature, humidity, pressure, wind and gust, rain, radiation); `APRSWX` without a call uses `default_call`. Optional periodic send, independent of the METAR. New `[aprs_wx]` section (free aprs.fi API key); without a key the feature is off. `APRSWX` rather than `APRS` keeps the latter free, e.g. for position queries.
- **Carrier accuracy and Hytera radios (PR #39, David Harcus, MM7FDM).** Clone SDR boards can transmit one or two kilohertz off frequency; Sepura radios register anyway, but **Hytera** radios (PT580H and similar) never find the cell (antenna crossed out, not one access burst in the station log) and no codeplug setting fixes it. New tool `tools/carrier-offset.py`: sweeps the carrier with an RTL-SDR on any nearby computer, finds the 25 kHz TETRA channel, reports the offset and suggests the `ppm_err` that centres it (ETSI limit 0.2 ppm, under 100 Hz at 438 MHz). Documented in `Docs/carrier-accuracy.md` and the README; the dashboard's PPM field help explains it. Seen on a B210 clone: carrier 1.56 kHz low, two Sepura registered, a PT580H Plus silent until the carrier was corrected to under 50 Hz.
- **Several networks in one station (`[networks]`, PR #38, David Harcus, MM7FDM) — tested on air 2026-11-07** (a 901/1638 `via` cell next to a 901/9999 cell on one station). A station can serve several TETRA networks at once, each with its own identity (MCC/MNC) and its own Brew core or none. Each cell is bound to one network (`network = "…"` at the root for the primary cell and in each `[[cells]]`) and broadcasts that network's identity, so a user who selects a network on the radio lands on that network's cells and is routed to its core: for example one carrier on TetraPack (901/9999) and another on another core (901/1234). A network without a core is local: its cells are switched inside the station (group and individual calls, SDS and roaming between them) with no server. Each network with more than one cell has its own site switch, hosted on the primary cell when it serves the network and otherwise on the network's lowest-numbered cell. Sibling cells of another network are announced as neighbours with their MCC/MNC. The Cells card shows each cell's network and the state of its link; `CellsSnapshot` adds `network` and `link_up`. Without a `[networks]` section nothing changes. Documentation in `Docs/networks.md`. Pending: routing by the radio's home network on a single carrier (MNI in U-LOCATION UPDATE DEMAND) and validation with two carriers and a radio with both networks programmed.
- **Multi Network: a network reached through the Brew server (`via`), editable on the dashboard.** Tested on air (radio selecting 901/1638 on a `via` cell, registered at TetraPack through the brew-server). Builds on "Several networks in one station" (PR #38, David Harcus, MM7FDM). A `[networks.<name>]` entry with `via = "<network>"` instead of its own `[brew]` is reached through that network's Brew server (Multi Network "Infrastructure"): its cells broadcast its own MCC/MNC (ETSI TS 100 392-2 table 18.17), connect with the other network's login, and name their radios and groups with this network towards a brew-server that offers `mni`, which registers them there, e.g. on its TetraPack link. So a radio whose home is 901/9999 that selects 901/1638 registers on that cell and works in 901/1638; the station holds no credentials of that network. New "Networks" card on the Infrastructure page (name, MCC/MNC, reached: own server, via, local) and a "Network" column per cell; the first network added turns `[net_info]` + `[brew]` into the network `home`, and removing a network moves its cells back to the primary cell's network in the same save. A radio of the server's own network may also migrate onto such a cell (with `[multi_network]` on; tested on air 2026-11-07). With several networks only the cells of a network count as its station's carriers for roaming, and the site switch of a network hosted on an extra cell uses that cell for its fallbacks.
- **Individual call (private/duplex) handover without interruption — provisional, not yet validated on air.** A private or duplex call no longer has to drop when the radio changes cell mid-call: the site switch pre-provisions the restored leg on the target cell (with a traffic channel allocated and open) and CMCE matches it by ISSI pair, answering `U-CALL RESTORE` with `D-CALL RESTORE` and a channel allocation that takes the radio to the new carrier. **Pending:** voice re-routing in the switch and cleanup of an aborted handover; not yet validated on multi-cell hardware. *(Completed in v0.5.6 for calls with a Brew party and between two radios of the station, announced or not; see "Multi-cell" there.)*
- **SDS delivery ticks in the console.** SDS-TL delivery reports a radio sends to the dispatcher's ISSI (or 9999) are no longer dropped as "not registered locally": they are matched to the sent message by its message reference, and the SDS log and the console's new "Sent" view show ✓ sent, ✓✓ delivered or ✗ rejected. Log entries keep `message_ref` and `delivered`; new telemetry event `SdsReport`.
- **Radio names.** "Name" button in the radios table and in the dispatch console list to give an ISSI a name (stored in `radio_names.json` next to the config, API `GET/POST /api/radio-names`). The name is shown wherever that ISSI appears, ahead of the RadioID callsign, and the receiving group chip shows who pressed PTT ("▶ Dave (2358245)"); the dispatch state includes `rx_issi`.
- **Scan list with names and import from file (dispatch console).** Each TG can carry a name and is shown as "World Wide (91)" in the chips and the call strip. "Import file" button (CSV, TXT or JSON: `World Wide (91)`, `World Wide,91`, `91,World Wide` or `{"name":"World Wide","gssi":91}`) and "Export" to CSV; optional name field when adding a TG by hand.
- **Fix: uplink slot numbering after "Stack is N slot(s) behind the SDR clock".** When the stack clock was advanced to catch up with the SDR, the uplink demodulators did not move with it, so from then on every burst from the radios was labelled one slot late: radio voice landed in a slot without a circuit (inactivity timer 3 s into every call, nobody in the group heard anybody), private calls never got set up and SDS between radios were never reassembled until a restart. Now the stack and the demodulators advance together: slots the demodulator skips because of lost samples are replicated in the stack without waiting for RX, and any other advance of the stack also skips one air slot in the uplink demodulators.
- **Radio authentication (TAA1, EN 300 392-7 clause 4).** The station can challenge each radio with its 128-bit key K at registration (`[security] authentication = off | optional | required`, with mutual authentication). Keys are stored per ISSI in `[[security.subscribers]]`. New crate `tetra-security` with HURDLE-II, TA11/TA12/TA21/TA22/TA41/TA51/TA52/TA61 and TB4/TB5, checked against public test vectors. Radios that pass show the **AUTH** badge in the radios table.
- **TEA2 and TEA3** in addition to TEA1 for class 2 encryption (`ksg = 2 | 3`). TEA1 stays for research and interoperability and is flagged as weak (it keeps only 32 bits of key).
- **SCK OTAR (clause 4.5.2).** Delivers the static key to a radio over the air, sealed with that radio's K (TA41 + TA51); the radio's answer (U-OTAR SCK RESULT) is shown in the panel. With `class = 1` and a key configured, the key is staged for OTAR only and the cell stays clear, so the fleet can receive the key before encryption is switched on.
- **Security page** in the panel: running vs saved state with one-click restart, authentication mode, subscriber key table with key generator, encryption (algorithm, SCK, number, version, clear groups), "Send SCK" per radio and to all connected radios; **Security** column in the radios table and SECURITY row in the sidebar.
- **USRP B210 (UHD):** "Install USRP (UHD)" option in the wizard and the installer, correct full-duplex defaults (TX on TX/RX, RX on RX2, PGA 40 dB), open FPGA image for the Kintex-7 clones (`fx3 is in state 5`), .deb package dependencies and documentation.
- **End-to-end encryption (E2EE) of SDS.** Off by default (`[security.e2ee]`): seals the content of status and text messages between radios with AES-128/256 in CTR mode and an AES-CMAC MAC (encrypt-then-authenticate), independent of TEA air encryption. Meant for private systems; on the amateur network it ships as a feature even if unused.
- **WAP page with live status.** The access point page shows real activity — SDS seen, overall signal level, registered radios and age of the last radio activity — with configuration documentation for operators. Adds a **per-radio table** that separates group activity from individual/private.
- **Fix: WAP packet data channel (PDCH, PR #37, David Harcus, MM7FDM) — provisional, not yet validated on air.** When granting the data channel, the station moved the radio to a slot that was still announced as unallocated and sent the WAP reply on the main control channel, which the radio no longer listened to, so the page never arrived. The slot is now announced as an assigned control channel while a radio is on it, and everything addressed to that radio (WAP reply, link acknowledgements, incoming individual calls and SDS) is sent on the data channel. The PDCH moves to slot 4 so as not to clash with voice, which is assigned from slot 2 and always has priority. SN-RECONNECT is answered, so a data session continues after a change of cell.
- **Fix: disabling the WAP no longer stops the station from starting.** `[cell_info.wap_ip].enabled` becomes the only master switch of the WAP; with the WAP disabled the BTS starts normally (before, it failed at startup).
- **Offline position map (LIP).** The map library (Leaflet) is served by the station itself, so the location map works without Internet access.
- **A config error no longer stops a running station.** When the extra cells are loaded, a cell with an invalid config is skipped with a warning instead of aborting the start of the whole station.

## v0.5.4 — Ambience listening (SS-AL) and location request

- **Ambience listening (SS-AL, ETSI EN 300 392-12-21).** New dispatch feature: the station accepts the `AmbienceListen` command from the control channel (brew-server) and recognises the ambience listening service byte the console sends. It sets up a direct simplex individual call to the target radio with the notification indicator "AL operation" (value 3, EN 300 392-9), so a supporting radio connects and opens its microphone on its own, without user action. The radio **indicates the call** like any other (it is not covert). The radio's audio reaches the console and, being a receive-only call, is no longer cut at 30 s. `enable: false` releases it. Validated on air with an MTH800.
- **Location request (LIP).** Dispatch can ask a radio for its position (once or periodically) and the LIP report reaches the console that asked for it. Before, with LIP forwarding active, the report went only to the forwarding ISSI and the console that asked never got it.

## v0.5.2 — Multi-cell: RF per cell and version in telemetry

- **RF page per cell.** Each extra cell now sends its spectrum, constellation, waterfall, signal quality (EVM, PAPR, carrier leakage, bandwidth) and SDR health (temperature, gains). Compact tabs at the top of the RF page choose the cell; each cell keeps its own waterfall history.
- **Station version in telemetry.** New event `StationVersion` (version, build and base version) on connecting to the telemetry server; brew-server shows it next to the BTS IP.
- **Less telemetry traffic.** Spectrum and constellation (~5 KB, five times a second per cell) are no longer sent to the telemetry server; only the local RF page uses them. EVM, PAPR and SDR health are still sent, per cell (`CellRf`).

## v0.5.1 — Multi-cell: network SDS and cell telemetry

- **Network SDS to radios on extra cells.** An SDS arriving over Brew (including brew-server's SMS Center) for a radio registered on an extra cell was dropped without an answer; now it is delivered on its cell and Brew gets the `SDS_REPORT`.
- **SDS between cells logged once.** The cell that receives it from another cell no longer logs it again as a network SDS in the SDS log or in telemetry.
- **Cell telemetry.** New event `CellsSnapshot` every 10 s (also on single-cell stations): carriers and frequencies, colour code, location area, neighbours, SDR, RF state and the radios registered on each cell. brew-server shows it in its telemetry panel and resyncs its registration list from it after a reconnect.

## v0.5.0 — Multi-cell: one station, several SDRs

First multi-cell version. One station, several SDRs: each SDR is one more TETRA cell (e.g. a Pi with two Pluto+). Complete in code and tests, **not yet validated on air**; single-cell configs work as in v0.4.4.

**Configuration and startup**
- New `[[cells]]` entries (id 1-7): inherit `[cell_info]`, carry their own `[cells.soapysdr]`. Unique carriers, same `freq_band` and `custom_duplex_spacing`, and `device` mandatory on every cell (the primary too). Single-cell configs do not change.
- Each cell runs its own radio stack on its own thread and SDR.
- Dashboard: **Cells** card on Home (RF state, carriers, SDR, radios) with SDR scan, adding and removing cells (restart).

**Linked cells (with Brew or LST active)**
- Group calls, from the network or from any radio, on every cell with members; one talker per group across the site; an emergency call takes over from a talker on another cell.
- Individual calls and SDS between cells; group SDS reaches members on every cell. Only what is for nobody on site goes out to Brew; groups in `local_ssi_ranges` link cells without going out to Brew.
- Asterisk (SIP) and WX/METAR from every cell; WX changes in the dashboard apply to all of them.
- Voice between cells goes through a jitter buffer and is played out at the TDMA pace of the receiving cell.

**Mobility**
- Each cell advertises the others as neighbours (announced and unannounced reselection), with carrier extension and maximum power where they differ.
- On registering on another cell, the radio is silently dropped on the old one and restores its group call on the new one.
- Announced handover: U-PREPARE → D-NEW-CELL (or D-PREPARE-FAIL); the target cell joins the radio's group calls in advance; the registration forwarded in U-PREPARE (type 1) is processed on the target cell and its answer goes inside D-NEW-CELL; U-RESTORE → D-RESTORE-ACK / D-RESTORE-FAIL.

**Dashboard and telemetry**
- The registered radios table shows every cell (badge C0/C1…) and the calls page includes the calls of every cell.
- Telemetry: new event `MsCell` (cell of each registration); calls on extra cells use station-own identifiers (from 0x4000).

**Changes for every station (single-cell too)**
- The station ignored all uplink MLE PDUs; now it answers U-PREPARE (D-PREPARE-FAIL if there are no neighbours) and U-RESTORE.
- D-NWRK-BROADCAST with neighbours carries configurable reselection thresholds/hysteresis (`[cell_info.cell_reselect]`, default 20/10/10/6 dB); before, always 0.
- `tetra-entities` tests repaired (`CallOrigin` import, Brew config in the preemption test) and `Cargo.lock` synced.

**Still to check on air:** bit order of the reselection parameters and of the carrier extension; individual calls that do not survive a cell change; CPU/bandwidth with several SDRs on a Pi.

## v0.4.4 — Dashboard ports: presets and stable binds

Port choice without fighting nginx/apache or spamming ERROR in the log.

- Installer (SSH/TTY): asks **Standard (80→443)** or **High port (HTTPS only, 8443)**; `BOST_DASH_PORTS=standard|high` without a TTY. Existing configs are not touched.
- System → Panel access: preset selector + Apply & Restart (new URL after restart).
- `port = 0` disables the HTTP redirect. OTA migration only if `port = 8080` without `https_port`.
- Legacy 8080/8443 listeners only in the :443 layout, fail-soft (no endless retries). Canonical bind: conflict → hint + retry 60 s.
- Validated in the field. Promoted to the **stable** channel (`main` / `bost`); `beta` at the same tip.

## v0.4.3 — LST SDS: hide delivery ACK in the log

- SDS-TL SHORT REPORTs (delivery confirmation from the MS) are no longer recorded in the SDS log/inbox.
- Avoids the ghost `[text]` row right after an SDS sent from dispatch.
- Validated in the field (MS↔dispatch private/group, origin `operator_issi`). Promoted to the **stable** channel (`main` / `bost`); `beta` at the same tip.

## v0.4.2 — LST SDS: dispatcher identity and ACK

LST dispatch SDS stops faking delivery to 9999 and uses the operator's ISSI.

- With LST active, SDS from the roster/panel go out with `source_issi = operator_issi` (not 9999).
- Incoming SDS to `operator_issi` are absorbed with an SDS-TL SHORT REPORT (the walkie stops flagging a send error).
- Blind absorption of SDS-DATA to 9999 removed: standard route (local / Brew / undeliverable). WX and U-STATUS to 9999 unchanged.
- LST inbox: only private messages for the dispatcher (no dual 9999 filter).

## v0.4.1 — Brew WiFi: leave mid-QSO like Ethernet

Same Ethernet/WiFi logic; with WiFi latency the MS could stay Local owner while Brew was talking, and red/TG change tore down the circuit (orphan media + U-SETUP on top).

- On Brew preempt, ownership Local→Network.
- U-DISCONNECT from the owner only soft-leaves if Brew has the floor (personal D-RELEASE; group stays up).
- No listeners + Brew active → Hold / LATE ENTRY (also if origin is still Local).
- Local release with brew_uuid → NetworkCallEnd before closing the circuit (anti-zombie DL).
- Owner with local floor: ETSI teardown unchanged.
- Validated in the field (WiFi Hold → LATE ENTRY). Promoted to the **stable** channel (`main` / `bost`); `beta` at the same tip.

## v0.4.0 — Host network, Dual Carrier GUI and stable channel

Minor series bump (no rebrand to PTBS yet): new network capabilities in the GUI and consolidation of what was validated in beta.

### Network page (formerly WiFi)

- **Network** menu with a hybrid Ethernet+WiFi icon (overlaid, LST style).
- **Links** section: ethernet/wifi interfaces with IP(s) and a **default route** badge.
- Management of **Ethernet** profiles (connect / disconnect) through NetworkManager.
- WiFi (current connection, saved networks, available) in **a single card** with separators.
- NM states and the "Wired connection" profile translated to the GUI language.
- API `/api/network/*` (overview + ethernet); `/api/wifi/*` unchanged.

### U-STATUS (walkie → ISSI 9999)

- `ip` / `info` list **all** host IPs (`eth0=…*`, `wlan0=…`; `*` = default route).
- **Multi-line** answer (CR/LF allowed in text SDS).
- Fast enumeration with `getifaddrs` (no `nmcli` on the radio thread — avoids a stack crash).

### Dual Carrier (TMO Cell)

- Dual Carrier configuration in the GUI (Config → Advanced RF), limited to the Fs passband.
- Home / BTS Details: Active/Off state, secondary mini-tiles, MCCH/BCCH order.

### Repo / OTA

- Removed the sync workflow with upstream FlowStation (force-push to `main`).
- Promoted to the **stable** channel (`main` / `bost`); `beta` at the same tip.

## v0.3.47 — Multi-line U-STATUS IP

- Text SDS allows CR/LF (they used to be filtered).
- Status IP: `Host IP` and each interface on its own line (`eth0=…*`, `wlan0=…`).
- Network page + U-STATUS IP fixes (v0.3.44–0.3.47) promoted to the **stable** channel (`main` / `bost`).

## v0.3.46 — Fix: U-STATUS IP does not block the stack

- The IP Status/info no longer calls `nmcli` on the radio thread (it caused "Too late to produce TX block" and a stack crash).
- Lists IPs with `getifaddrs` + `primary_ip()` (fast, like the temperature Status).

## v0.3.45 — Network: icon, NM i18n and unified WiFi

- Network menu icon: Ethernet jack and WiFi arcs overlaid (LST style), wider arcs.
- Page section "Network" (formerly "Integrations"); WiFi in a single card with separators.
- NM states (`connected`, …) and the "Wired connection" profile translated to the GUI language.

## v0.3.44 — Network page: Ethernet + WiFi

- The **WiFi** tab is renamed **Network** (hybrid Ethernet+WiFi icon).
- New **Links** section: every ethernet/wifi interface with IP and a **default route** badge.
- Management of **Ethernet** profiles (connect / disconnect) through NetworkManager.
- The WiFi block (connection, saved, scan) stays below.
- U-STATUS `ip` / `info` list the IPs per interface (`eth0=…* wlan0=…`; `*` = default route).

## v0.3.43 — Dual Carrier: mini-tile borders + stable promotion

- Carrier / TX / RX / Duplex frames inside Dual Carrier a bit darker (better contrast).
- Dual Carrier GUI (v0.3.38–0.3.43) promoted to the **stable** channel (`main`).

## v0.3.42 — Dual Carrier: same background as the rest

- The Dual Carrier card uses the same flat background (light theme) as Registration Access / BTS tiles.

## v0.3.41 — Dual Carrier: a single card

- Carrier / TX / RX / shift of the secondary go **inside** the Dual Carrier card (no separate box).
- State: **Active** (bold green) / **Off** (bold orange); the text "secondary carrier #…" is removed.

## v0.3.40 — Dual Carrier UI: TS order + mini-tiles

- In BTS Details, the **MCCH (main)** row goes on top and the **BCCH secondary** below.
- The secondary's mini-tiles (carrier, TX, RX, shift) appear **under Dual Carrier** when it is active (Home already fills them).

## v0.3.39 — OTA fix: encoding in server.rs

- Fixes corrupt UTF-8 literals in the dashboard's mojibake detector that stopped `tetra-entities` from compiling during OTA (v0.3.38).

## v0.3.38 — Dual Carrier in TMO Cell (GUI)

- Dual Carrier is configured in **Config → Advanced RF** (checkbox under Main carrier; the secondary only appears when ON).
- The secondary is **limited to the passband** of the SDR's real Fs (or 600 kHz by default); enabling it saves Fs + midway centres to TOML and the Cell profile.
- In **TETRA BTS Details**: "Configure…" button (no switch) and **mini-tiles** of the secondary (no., TX, RX, shift) when it is active.

## v0.3.37 — Mid-report overflow → immediate D-ATTACH

- If the U-ATTACH carries *group report not complete* and >12 GSSIs (MXP600), after the ACK for 12 the rest are affiliated at once with a **SwMI D-ATTACH** (no new group report is requested).
- If the following amendment arrives truncated (`BufferEnded` on parsing), the rest saved from the previous PDU is used for the same D-ATTACH.

## v0.3.36 — SwMI attach fallback if the MS does not do multi-pass

- After a group report (§16.8.3), if the MXP600 again sends >12 GSSIs in a single U-ATTACH (no multi-pass amendment), the BTS affiliates the rest and sends a SwMI **D-ATTACH amend** (§16.8.1) with those GSSIs. IOP may ignore it; if it stays at 12, limit the scan list to ≤12.

## v0.3.35 — ETSI multi-pass affiliation (§16.8.3)

- If a U-ATTACH carries more than **12** GSSIs (e.g. MXP600 scan list), after the ACK for the 12 the BTS asks for a SwMI **group report** (EN 300 392-2 §16.8.3) so the MS re-affiliates in several messages (detach-all + amendments).
- Log fixed: it no longer says the MS will retry on its own. Validate on air: scan >12 → more than 12 in "Affiliated groups". If the MS does not do multi-pass after the report, limit the scan list to ≤12.

## v0.3.34 — Restart recovery in Config

- Switch **Restart recovery (proactive)** in Advanced network/timers (live config + TMO Cell profile), with "?" help. Default **off**. After Apply and restart with the check on, the BTS re-registers cached ISSIs without touching the walkie. Reactive recovery (on PTT/TG) stays ON in the engine.

## v0.3.33 — Pending-tail quiet 400 ms

- Fine tuning: Brew post-drain quiet **400 ms** (was 550) after `GROUP_IDLE`.

## v0.3.32 — Brew: do not cut the last syllable when the remote PTT is released

- After `GROUP_IDLE` the `NetworkCallEnd` is **always deferred** (even if the Brew jitter buffer is empty) so UMAC finishes transmitting the last TCH.
- Post-drain quiet ~**400 ms** (tuned in 0.3.33; was 550 / before that 150); keeps the hang time from "swallowing" the tail and releasing it when the next PTT opens.

## v0.3.31 — Brew late entry: DL audio + Hold when leaving the TG

- **Brew audio:** new network calls open the circuit in **SwMI** (before LocalLoopback, meant for LST; remote audio did not go on air).
- **TG change mid-QSO:** if the last walkie leaves the GSSI, the Brew session is **held** (Hold) instead of End; on affiliating again it is rebuilt + D-SETUP.

## v0.3.30 — OTA compile fix (late entry)

- Visibility of `push_control` across CMCE modules + exhaustive match in UMAC for the new late entry SAPs (the release build failed in 0.3.29).

## v0.3.29 — Usable late entry (QSO in progress)

- **Brew:** if a GROUP_TX arrives without affiliated walkies, the call is **held** (pending) instead of dropped; on the first Affiliate the circuit + D-SETUP + audio are set up.
- **Affiliate / TG change:** immediate D-SETUP if there is already a QSO on that GSSI (no waiting ~5 s).
- **LST dispatch:** on affiliating a TG with an active QSO, the console hooks RX (`rx_gssi` green + audio) without new texts.
- **`late_entry_supported`** defaults to **true** in `[cell_info]`; checkbox in Config (Advanced cell) next to System-wide services.

## v0.3.28 — Soft site trunking (DIMETRA / TIP style)

- After a Brew blip all walkies are **no longer** kicked out with `D-LOCATION-UPDATE-COMMAND`.
- On reconnect: subscriber resync to the core (REGISTER/AFFILIATE); COMMAND only **on demand** to one ISSI if a setup via Brew fails within the soft-recovery window.
- Backhaul hysteresis (default **3 s**): short 4G/5G blips do not change the "local area only" menu. Brew calls are released at once; local groups on the cell carry on.
- New `[brew] backhaul_hysteresis_secs` (0..=60).

## v0.3.27 — TetraPack: no re-registration on the first GROUP_TX

- If the core does not announce a version in the handshake (TetraPack), the first call with a mnemonic no longer triggers `BrewReconnected` / `D-LOCATION-UPDATE-COMMAND`.
- That sweep only happens after a real backhaul disconnect→reconnect (still covers PTT denied after a blip).

## v0.3.26 — OTA: RF OFF at the start

- When an OTA update starts the SDR is switched off at once (before compiling), so radios lose the cell cleanly. The final restart opens RF again from the config.

## v0.3.25 — LIP location modal (mobile)

- Each radio keeps only the **Center** button (the duplicated "Center all" text in the card is removed).
- The top **Center all** button is centred, bigger and bold.
- Modal title vertically aligned with the close button (mobile and PC).

## v0.3.24 — OTA fix: borrow in lip_forward_issi

- Fixes E0716 in `brew_routable` (temporary dropped while borrowed) that blocked the OTA build of 0.3.23.

## v0.3.23 — LIP forwarding → Brew + stable dashboard

- **Brew:** in Advanced (profile and live settings), **LIP forwarding** + **target ISSI** right under RSSI export. Every UL LIP (PID 10) is forwarded to that ISSI over Brew, whatever the radios say.
- **Dashboard:** the WebSocket no longer takes a slot of the 32 HTTP connection cap; avoids API/WS timing out after a while and needing a restart.

## v0.3.22 — LIP location: Center all only where it belongs

- **Center all** removed from the modal header.
- On desktop it stays in the header of the actions column; on mobile, between the map and the cards.
- The per-radio **Center** does not change.

## v0.3.21 — Adaptive LST audio + Geo without leaks

- **DL PCM:** only with dispatch taken. In a call/RX active it goes back to **80 ms** (latency); idle drops to **500 ms**.
- **LIP location:** closing the modal destroys the Leaflet map (stops requesting OSM tiles) and `/api/lst/positions` is not queried again until it is reopened.
- Geo modal: redundant bar removed (Center all + ISSI state); **Center** / **Center all** centred in the actions column (on mobile, Center all moves to the header).

## v0.3.20 — Lighter dashboard (Pi)

- Cap of **32** concurrent HTTP(S) connections: keeps aggressive polling / retries from bringing the process down (ERR_CONNECTION_RESET).
- Fewer re-renders for RSSI (debounce 250 ms); timeslot timers 150→250 ms.
- Callsigns / LST status / Geo / service: do not hammer the API if the tab is in the background or the link is down.

## v0.3.19 — LIP location modal

- Title **LIP location** (formerly Geo LIP).
- **Center all** moves to the header of the actions column; on mobile it is shown above the table (the thead stacks).
- Refresh removed (the modal already refreshes every 5 s).
- Close (×) no longer overlaps the title separator on mobile.

## v0.3.18 — Type filter in the SDS log

- Selector All / LIP / Text / Status / Concat / Home / Other next to Export.
- Refresh removed: the log loads when the tab opens and arrives live over WebSocket.

## v0.3.17 — Location on Home (without LST)

- **Location** button in the Registered radios card (Home): same Geo LIP modal as in LST.
- The LIP position store lives in the dashboard (does not depend on the LST Dispatch profile).

## v0.3.16 — Location button in the LST roster

- The **Location** button moves to the Online radios card (replaces the manual Refresh, redundant with the automatic poll).

## v0.3.15 — Geo LIP marker

- Own map pin (CSS, dashboard accent colour); no longer depends on Leaflet's broken PNGs/CDN.
- Correct RadioID callsign in the Geo table.

## v0.3.14 — Geo LIP in LST dispatch

- Decoded LIP positions (SDS PID 10, UL) feed the LST store (`note_position`).
- **Geo** button in the LST console: modal with table + OpenStreetMap map (Leaflet lazy, only when opened).
- Coordinates 0,0 are ignored (initialisation handshake). Needs the LST Dispatch profile active.

## v0.3.13 — LIP decode + remaining Miura

- **LIP:** short reports (SDS PID 10) are decoded to `LIP position: lat, lon` (ETSI TS 100 392-18-1). GeoAlarm/Telegram and the SDS log no longer see the empty payload.
- **Miura (rest):** `mon_pattern` / MPN 1 in channel allocation (long PTT on Sepura); D-RELEASE also over FACCH on the traffic timeslot.

## v0.3.12 — OTA bridge to PTBS

**Do OTA once** (Stable or Beta channel). This version prepares the migration to the **PTBS** product (*Personal Tetra Base Station*).

- The **Stable** channel now follows git branch **`main`** (before `bost`). Branch `bost` still receives this bridge so current installs can update.
- OTA recognises checkout `/opt/ptbs` (preferred) and `/opt/bost-flowstation` (legacy).
- Installing the binary also refreshes `/usr/local/bin/ptbs` next to `bluestation-bs`.
- Rebrand announcement in the README and OTA messages. The UI brand stays Bost FlowStation until the **0.4.0** cut.
- Migration window: keep the device updated; in a few days the repo/branches/binary will complete the rename to PTBS.

## v0.3.1

- **Affiliated groups:** the chevron (›) panel no longer disappears at once. It stays open until closed (×, click outside, Escape or the chevron again). Before, the roster refresh, scrolling and a 3.5 s timer closed it; the button's native `title` was also confusing on mobile.

## v0.3.0

Stable release (OTA channel **Stable** / branch `bost`). Consolidates the work of the 0.2.4–0.2.41 line: LST dispatch in production, PTT preempt, canonical HTTPS dashboard and field fixes.

### Updating from 0.2.x (stable)

- OTA to **Stable** / `bost` or reinstall with `install-bost.sh` (does not overwrite an existing `config.toml`).
- On startup, if the dashboard was still on the old ports (`port = 8080`), it is migrated to `port = 80` + `https_port = 443`. Open **`https://<IP>/`**.
- Silent redirects on `:8080` / `:8443` are kept only for old bookmarks; **install and UI no longer advertise those ports**.
- LST voice codec: if missing, OTA offers a rebuild with libtetra-codec (no SSH).

### LST dispatch (local console)

- Console under Integrations: session claim, dispatcher ISSI, multi-TG scan list, PTT (mouse/touch/space), SDS, roster, activity and SDS inbox.
- Simplex/duplex private calls (outgoing and incoming); call modal/strip.
- ACELP audio through the OTA codec; canonical dashboard on HTTPS `:443` (HTTP `:80` redirects).
- **Preempt / interruption:** LST PTT can take the floor from a local MS (deny → offer → Ready on UL quiet); no circuit teardown; no Motorola display flicker after Ready.
- After a group PTT, the SX/DX private dial no longer stays stuck as "Established" with the GSSI.
- Incoming call modal only in the browser that has dispatch taken (does not bother other dashboard agents).

### Dashboard / Config / network

- HTTPS dashboard `:443` + HTTP redirect `:80` (installer and docs aligned).
- "?" help in Config (TETRA/Brew timers); ISSI whitelist in the Cell profile; TOML fix with 2+ ISSIs (avoids starting in fallback).
- Wi-Fi resilience (NM drop-in, autoconnect, watchdog) since 0.2.2–0.2.3.
- LST/mobile UX polish, navigation icons, Cell × Brew profiles.

### Cleanup in 0.3.0

- Removed the "Open secure console (HTTPS)" button from dispatch (the UI is already served over HTTPS).
- Installer / README / example_config messages no longer publish `:8080` / `:8443`.

## v0.2.7

- **LST Dispatch:** admit group `NetworkCallStart` without Brew (inbound gate). Join only selects the GSSI; PTT opens/closes the call. SDS uses the dispatch ISSI (`source_issi` / `dest_is_group`). Private calls: media ready, duplex UL, visible errors.

## v0.2.6

- Fix LST PTT borrow-checker errors so `tetra-entities` compiles on OTA.

## v0.2.5

- Fix OTA build: import `CfgLstDispatch` / `apply_lst_dispatch_patch` in tetra-config.

## v0.2.4

- **LST Dispatch:** local console under Integrations (browser mic/speaker). Immutable Brew profile "LST Dispatch", 1 session, XOR with real Brew. Group + simplex/duplex private calls + SDS + roster; voice codec if the build includes `asterisk`.

## v0.2.3

- OTA / startup apply the NetworkManager drop-in `bost-wifi.conf` (powersave off) on their own: **no SSH needed**.

## v0.2.2

- **WiFi resilience:** Disconnect uses `connection down` (no longer inhibits autoconnect). On connect, autoconnect + powersave off are forced on the profile. A light watchdog brings a saved profile back up if the link drops with the WiFi radio on.
- Installer: NetworkManager drop-in `bost-wifi.conf` (`wifi.powersave=2`). Check docs in install-and-setup.

## v0.2.1

- U-STATUS config: on PC the commands go back to a single row (code | action | Remove); the compact mobile layout does not change.

## v0.2.0

Stable release with product changes (not just patches). OTA channel **Stable** (`bost`).

- **Mobile dashboard:** shell, System/OTA/Setup, tables, RF/Health, Config, DGNA/Geoalarm/Wi-Fi and integrations usable on a phone (PC layout unchanged except where agreed).
- **More robust OTA:** modal at once, coalesced and cached checks, purge of an empty `.git`, clear messages, post-restart wait that requires "down → up" and leads to login when auth is on (avoids the SPA being "offline").
- **Config / profiles:** Advanced network timers with reset to default (empty = engine default); Live and Cell profiles share the same persistence semantics.
- **Home:** quick Cell × Brew profiles; UI polish (Save at the bottom, empty messages, compact U-STATUS, etc.).

## v0.1.57

- Installer aligned with OTA: fetch with refspec + retries, `reset --hard`, and `ota_channel` per `BOST_BRANCH`.
- README / install docs updated (the curl command is still always from branch `bost`).

## v0.1.56

- OTA: `git fetch` retries on TLS/network drops (e.g. GnuTLS on a Pi).

## v0.1.55

- OTA progress step: short top status, a different tip under the bar and elapsed time in bold.

## v0.1.54

- Human-readable release notes on the update screen (CHANGELOG / Releases).
- The "update available" banner opens the change summary directly.
- OTA modal with a clearer step indicator (channel → notes → progress).

## v0.1.53

- Build fix in the OTA permissions helper (`append` / `&str`).

## v0.1.52

- After syncing the code as root, ownership of the whole source tree is fixed
  (not only `target/`) so `cargo` as user `bts` does not fail on `Cargo.lock`.

## v0.1.51

- On changing channel (e.g. to Beta), the fetch creates `origin/<branch>` correctly
  so the update does not fail with "unknown revision".

## v0.1.50

- Three-step OTA dialog: choose the channel, see the notes and confirm, then progress.
- The channel selector is saved and still feeds the automatic badge / banner.

## v0.1.49

- Fix for a build error in the OTA channel configuration.

## v0.1.48

- OTA channels **Stable** (`bost`) and **Beta** (`beta`).
- Safe sync with `git reset --hard` (recovers force-pushes) keeping `target/`
  for incremental builds.
- If the binary already matches HEAD, it is not rebuilt or falsely restarted.
