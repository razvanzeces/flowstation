# Inventario semántico de `config.toml` (FlowStation 0.4 / schema 0.6)

Mapa campo → significado → grupo de perfil → restart. Fuente: `crates/tetra-config` + `example_config/config.toml`.

## Modelo de perfiles (BTS Mia → FlowStation)

| Catálogo | Contiene | Ortogonal a |
|---|---|---|
| **Cell** (RF + Network/Celda) | `phy_io`, `net_info`, `cell_info` (operativo) | Brew |
| **Brew** | `[brew]` o ausente (= offline) | Cell |
| **Station / Integrations** | dashboard, security, telegram, dapnet, … | Quedan en el TOML vivo; apply de perfiles no los pisa |

Apply = combinar Cell × Brew opcional → fusionar en `config.toml` → validar → backup `.bak` → reiniciar.

## Convención restart

- **Sí**: leído solo al boot del stack.
- **No**: override runtime del dashboard (`SharedConfig::effective_*`).

---

## RF (`phy_io` + portadora en `cell_info`)

| Path | Tipo | Default | Significado | Restart |
|---|---|---|---|---|
| `phy_io.backend` | enum | req | `SoapySdr` | Sí |
| `phy_io.soapysdr.tx_freq` | f64 | req | DL Hz | Sí |
| `phy_io.soapysdr.rx_freq` | f64 | req | UL Hz | Sí |
| `phy_io.soapysdr.ppm_err` | f64 | 0 | Corrección PPM | Sí |
| `phy_io.soapysdr.device` | str? | — | Args Soapy | Sí |
| `phy_io.soapysdr.rx_antenna` / `tx_antenna` | str? | — | Antenas | Sí |
| `phy_io.soapysdr.sample_rate` | f64? | — | Fs (obl. dual-carrier) | Sí |
| `phy_io.soapysdr.rx_gain_*` / `tx_gain_*` | map | — | Ganancias por elemento | Sí |
| `phy_io.soapysdr.rx/tx_center_freq` | f64? | — | Centro SDR multi-carrier | Sí |
| `phy_io.*_file` | path? | — | Captura/replay debug | Sí |
| `cell_info.freq_band` | u8 | req | Banda SYSINFO (4 = 400 MHz) | Sí |
| `cell_info.main_carrier` | u16 | req | Portadora principal | Sí |
| `cell_info.secondary_carrier` | u16? | — | Dual-carrier | Sí |
| `cell_info.dual_carrier_enabled` | bool | true si ausente | Switch dual-carrier | Sí |
| `cell_info.duplex_spacing` | u8 | req | Índice tabla duplex | Sí |
| `cell_info.custom_duplex_spacing` | u32? | — | Duplex custom Hz | Sí |
| `cell_info.freq_offset` | i16 | req | 0 / ±6250 / 12500 | Sí |
| `cell_info.reverse_operation` | bool | req | UL por encima de DL | Sí |
| `cell_info.ms_txpwr_max_cell` | u8 | 4 | Potencia máx MS 0–7 | Sí |
| `cell_info.rxlev_access_min_dbm` | i16 | -110 | RXLEV_ACCESS_MIN (dBm, tabla 21.65) −125…−50 en pasos de 5 | Sí |
| `cell_info.colour_code` | u8 | 0 | Colour code SYNC | Sí |

**Validación clave:** `tx_freq`/`rx_freq` deben coincidir con DL/UL derivados de `FreqInfo` (banda + carrier + duplex ± reverse).

**Auto (UX):** banda 4, no reverse: `main_carrier = (tx_freq - 400e6 - offset) / 25000`; `rx_freq = tx_freq - duplex_hz`.

---

## NetworkCell (`net_info` + resto operativo de `cell_info`)

| Path | Tipo | Default | Significado | Restart |
|---|---|---|---|---|
| `net_info.mcc` | u16 | req | MCC | Sí |
| `net_info.mnc` | u16 | req | MNC | Sí |
| `cell_info.location_area` | u16 | req | LA | Sí |
| `cell_info.system_wide_services` | bool | false | System-wide (fallback sin Brew) | Sí |
| `cell_info.voice_service` | bool | true | Voz | Sí |
| `cell_info.local_ssi_ranges` | [[u32;2]] | [[0,90]] | SSI locales (fin inclusivo) | Sí |
| `cell_info.timezone` | str? | — | IANA → D-NWRK-BROADCAST | Sí |
| `cell_info.hangtime_secs` | u32 | 5 | Hangtime grupo 0–300 | Sí |
| `cell_info.call_timeout_secs` | u32 | 120 | T310; 0=∞ | Sí |
| `cell_info.ul_inactivity_secs` | u32 | 3 | UL silence → TX-CEASED | Sí |
| `cell_info.periodic_registration_secs` | u32 | 3600 | T351; 0=off | Sí |
| `cell_info.presence_check_secs` | u32 | 30 | Idle-radio challenge; 0=off, otherwise 5–3600 s | No (TOML/JSON) |
| `cell_info.presence_response_secs` | u32 | 30 | Response grace; 10–300 s | No (TOML/JSON) |
| `cell_info.home_mode_display.*` | bloque? | — | Callsign SDS PID 220 | Sí |
| `cell_info.sds_broadcast.*` | bloque? | — | SDS periódico extra | Sí |
| `cell_info.neighbor_cells_ca[]` | ≤7 | — | Vecinos CA | Sí |
| `cell_info.dgna_*` | varios | — | DGNA OTA | Sí |
| flags servicio (registration, sndcp, …) | bool | varios | Anuncio SYSINFO | Sí |

---

## Brew

| Path | Tipo | Default | Significado | Restart |
|---|---|---|---|---|
| `[brew]` ausente | — | offline | Sin backhaul | Sí |
| `brew.host` | str | req | Host | Sí |
| `brew.port` | u16 | 443 | Puerto (Pi usa 3003 en despliegues TetraPack) | Sí |
| `brew.tls` | bool | req | WSS/HTTPS | Sí |
| `brew.username` | u32 | req | SSID Digest | Sí |
| `brew.password` | secret | req | Password | Sí |
| `brew.reconnect_delay_secs` | u64 | 15 | Reconnect | Sí |
| `brew.jitter_initial_latency_frames` | u8 | 0 | Jitter playout | Sí |
| `brew.feature_sds_enabled` | bool | true | SDS↔Brew | Sí |
| `brew.feature_rssi_export` | bool | false | RSSI 0xf4 | Sí |
| `brew.whitelisted_ssis` | [u32]? | — | Filtro outbound | Sí |
| `brew.pbx_gateway_issis` | [u32]? | — | Gateways PBX | Sí |

---

## Station (no van en perfiles Cell/Brew)

`config_version` (debe `"0.6"`), `stack_mode`, `debug_log`, `service_name`, `[dashboard]`, `[health]`, `[recovery]`, `[emergency]`, `[wx_service]` (runtime No).

## Security

`issi_whitelist` (**No** restart lista), `whitelist_mode` (Sí), rate limits / max clients (Sí).

## Integrations (formularios ya existentes salvo Asterisk)

| Sección | Restart |
|---|---|
| `telegram_alerts` | No (salvo `alert_health`) |
| `dapnet`, `geoalarm`, `snom_notify` | No |
| `tpg2200_action`, `asterisk`, `telemetry`, `command` | Sí |

## Advanced

`cell_info.sds_command_control` (U-STATUS → restart/shutdown/kick_all) — Sí.

---

## Validaciones globales (`StackConfig::validate`)

1. Backend Soapy coherente.
2. `secondary_carrier ≠ main_carrier`.
3. Frecuencias Soapy ↔ `FreqInfo`; dual-carrier passband + sample_rate.
4. `ms_txpwr_max_cell` 0–7; `rxlev_access_min_dbm` −125…−50 en pasos de 5; timezone IANA; ≤7 vecinos únicos.
5. Claves desconocidas → reject en parsing.
