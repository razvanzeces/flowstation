// ---------------------------------------------------------------------------
// TelemetryEvent — concrete enum sent through the channel
//
// Small, hot-path variants are inline (no heap allocation).
// Rare / large variants use heap-allocated payload so the enum stays small.
// ---------------------------------------------------------------------------

use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct MsGroupInfo {
    pub gssi: u32,
    pub mnemonic: Option<String>,
    pub attachment_mode: Option<u8>,
    pub is_dynamic: bool,
    pub is_attached: bool,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct DgnaStatusInfo {
    pub issi: u32,
    pub gssi: u32,
    pub attach: bool,
    pub accepted: bool,
    pub source: String,
    pub detail: String,
}

/// TelemetryEvent enum sent by a TetraEntity through the TelemetrySink
/// then, serializable by any codec for transmission over the network,
/// using any Transport.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum TelemetryEvent {
    /// MS registered on BS
    MsRegistration { issi: u32 },
    /// MS deregistered. Also counts as deregistration for all groups.
    MsDeregistration { issi: u32 },
    /// MS dropped because it did not answer the periodic registration (T351). Emitted in
    /// addition to `MsDeregistration` for the same ISSI — consumers that distinguish the reason
    /// (e.g. Telegram alerts) should coalesce the two. LIP/APRS position beacons are detected
    /// separately from `SdsLog { protocol_id: 10 }`, so no dedicated event is needed for them.
    MsTimeoutDrop { issi: u32 },
    /// MS affiliated to groups
    MsGroupAttach { issi: u32, gssis: Vec<u32> },
    /// Full snapshot of all currently attached groups — emitted after any attach/detach
    MsGroupsSnapshot { issi: u32, gssis: Vec<u32> },
    /// Full per-MS group catalog, including DGNA/static classification and detached DGNA records.
    MsGroupCatalogSnapshot { issi: u32, groups: Vec<MsGroupInfo> },
    /// Backend status for an operator DGNA request. Emitted by MM for acceptance/rejection and by
    /// CMCE/SS for air-interface progress and SS-DGNA ACK outcomes.
    DgnaStatus(DgnaStatusInfo),
    /// MS detached from groups
    MsGroupDetach { issi: u32, gssis: Vec<u32> },
    /// RSSI measurement for a known MS (dBFS)
    MsRssi { issi: u32, rssi_dbfs: f32 },
    /// Group call started. `priority` is the ETSI call priority (0..=15) from the originating
    /// U-SETUP / network call start; 15 denotes an emergency call (`priority` appended last so
    /// existing leading fields stay wire-stable for the bitcode codec).
    GroupCallStarted {
        call_id: u16,
        gssi: u32,
        caller_issi: u32,
        carrier_num: u16,
        ts: u8,
        priority: u8,
    },
    /// Group call ended
    GroupCallEnded { call_id: u16, gssi: u32 },
    /// Active speaker changed on a live call. `dest_addr` is the active destination
    /// (GSSI for group, peer ISSI for individual/simplex).
    CallSpeakerChanged {
        call_id: u16,
        is_group: bool,
        dest_addr: u32,
        speaker_issi: u32,
        carrier_num: u16,
        ts: u8,
    },
    /// Individual (P2P) call started. `priority` is the ETSI call priority (0..=15) from the
    /// originating U-SETUP; 15 denotes an emergency call (appended last for bitcode wire-stability).
    IndividualCallStarted {
        call_id: u16,
        calling_issi: u32,
        called_issi: u32,
        simplex: bool,
        carrier_num: u16,
        ts: u8,
        peer_carrier_num: Option<u16>,
        peer_ts: Option<u8>,
        priority: u8,
    },
    /// Individual call ended
    IndividualCallEnded { call_id: u16 },
    /// Energy saving mode updated for MS (0=StayAlive, 1=Eg1..7=Eg7)
    MsEnergySaving { issi: u32, mode: u8 },
    /// Brew (TetraPack) backhaul connection status changed
    BrewConnected { connected: bool, server_version: u8 },
    /// SDS message activity (local delivery or group)
    SdsActivity { source_issi: u32, dest_issi: u32 },
    /// One SDS message handled by the BS, for the dashboard SDS Log tab. `direction`:
    /// "rx" = uplink received from a local MS over the air, "net" = arrived from the
    /// network (Brew/SwMI) for local delivery, "tx" = injected by the dashboard operator.
    /// `text` is the best-effort decoded message body (empty for status/report/binary
    /// payloads); `protocol_id` is the leading SDS protocol-identifier byte.
    SdsLog {
        direction: String,
        source_issi: u32,
        dest_issi: u32,
        is_group: bool,
        protocol_id: u8,
        text: String,
        /// SDS-TL message reference of a text message (None for other payloads), used to match a
        /// later delivery report to this entry.
        message_ref: Option<u8>,
    },
    /// SDS-TL delivery report from a radio for a message the dashboard / dispatch console sent:
    /// `source_issi` is the radio reporting, `dest_issi` the console ISSI it reports to,
    /// `delivered` false for "memory full" / "not supported".
    SdsReport {
        source_issi: u32,
        dest_issi: u32,
        message_ref: u8,
        delivered: bool,
    },
    /// Voice frame activity on a traffic timeslot (UL or DL)
    TsVoiceActivity {
        carrier_num: u16,
        ts: u8,
        speaker_issi: Option<u32>,
    },
    /// Fast visual feed for the RF dashboard: spectrum + constellation + RMS/peak.
    /// Emitted ~5 times per second so spectrum/constellation/waterfall feel fluid.
    /// Cheap to compute (FFT + magnitude). Constellation symbol recovery is the
    /// only non-trivial bit, but it's still well under 1 ms on a Pi 5.
    ///
    /// Works on any radio (LimeSDR, SXceiver, µCell, USRP, Pluto) because the
    /// analysis runs on the complex baseband samples FlowStation generates
    /// internally, BEFORE they reach the SDR — no receive-side feedback required.
    TxVisual {
        sample_rate: f32,
        center_freq_hz: f64,
        /// RMS amplitude in dBFS (0 = full scale). Shown smoothed in the UI.
        rms_dbfs: f32,
        /// Peak amplitude in dBFS. Shown smoothed in the UI.
        peak_dbfs: f32,
        /// 512-bin spectrum, magnitude in tenths of a dB (i16 to keep the WS message compact).
        spectrum_db_tenths: Vec<i16>,
        /// Recovered symbol-rate IQ samples, interleaved I,Q,I,Q,... scaled to fit i16.
        constellation_iq: Vec<i16>,
        /// Configured BS downlink carriers as (carrier number, absolute frequency Hz).
        /// The RF page uses these to place carrier-aware markers over the aggregate
        /// pre-PA spectrum/waterfall.
        carriers: Vec<(u16, f64)>,
        /// Carrier selected for constellation recovery, as (carrier number, absolute
        /// frequency Hz). In multi-carrier mode the aggregate SDR-centered samples
        /// must be mixed down before IQ/EVM is meaningful.
        constellation_carrier: Option<(u16, f64)>,
    },
    /// Slow, expensive signal-quality metrics. Emitted once per second so the
    /// numbers on the RF dashboard sit still instead of flickering. The values
    /// are aggregates over multiple symbol blocks — averaging is done in the DSP,
    /// not in the browser, so a single message is already a stable reading.
    TxQuality {
        /// Peak-to-Average Power Ratio in dB. Typical π/4-DQPSK target ≈ 3.5-4 dB.
        /// Higher values indicate clipping or modulation problems.
        papr_db: f32,
        /// RMS-normalized Error Vector Magnitude as a percentage. Per 3GPP TS 36.104
        /// methodology: compare measured IQ to ideal constellation, normalize by RMS
        /// of ideal symbols. Lower is better (≤5% professional, ≤12% acceptable).
        evm_pct: f32,
        /// Mean of the I component across captured samples (DC offset on I).
        /// Should be ~0; non-zero indicates DC bias from the SDR front-end.
        dc_offset_i: f32,
        /// Mean of the Q component (DC offset on Q).
        dc_offset_q: f32,
        /// Amplitude imbalance between I and Q in dB. 0 dB = balanced.
        iq_amplitude_imbalance_db: f32,
        /// Phase imbalance between I and Q in degrees (deviation from ideal 90°).
        iq_phase_imbalance_deg: f32,
        /// Carrier (LO) leakage in dB relative to total signal power. More negative
        /// is better. Direct-conversion SDRs (SXceiver, µCell) typically show this.
        carrier_leakage_db: f32,
        /// Occupied bandwidth in Hz — width containing 99% of total power.
        occupied_bandwidth_hz: f32,
        /// Carrier used for the EVM calculation, as (carrier number, absolute
        /// frequency Hz). None means the aggregate/DC signal was used.
        evm_carrier: Option<(u16, f64)>,
    },
    /// SDR hardware health snapshot. Emitted every ~5 seconds. Some fields may be
    /// absent (None) depending on what the radio exposes via Soapy.
    SdrHealth {
        /// Sensor reading in °C if the device exposes it (LimeSDR ✓, USRP ✓, Pluto ✓, SXceiver ✗)
        temperature_c: Option<f32>,
        /// Actually-set TX gain values per gain stage, queried back from the radio.
        /// Vec of (name, dB) — e.g. [("PAD", 40.0), ("IAMP", 6.0)] for LimeSDR.
        tx_gains: Vec<(String, f32)>,
        /// Same for RX gain stages.
        rx_gains: Vec<(String, f32)>,
    },
    /// Host system health: temperatures, voltages, currents, power consumption.
    /// Aggregated from whatever sysfs exposes — works on RPi 5 (full PMIC),
    /// x86 with RAPL, RPi 4 (CPU temp only), laptops (battery), and degrades
    /// gracefully on anything else. Emitted every ~2 seconds.
    SysHealth {
        /// Estimated total system power draw in watts, if we can compute it.
        /// On RPi 5 this is the sum of all PMIC rails (~5-12W typical).
        /// On x86 it's RAPL package power (CPU only, not whole system).
        /// On laptop/battery devices it's the battery discharge rate.
        /// None when no power-capable source is detected.
        total_power_w: Option<f32>,
        /// Individual sensor readings, in display order.
        sensors: Vec<SysSensor>,
    },
    /// Lite stack-health roll-up (Service / Backhaul / Radios / Congestion), emitted by the
    /// health monitor every few seconds. Rendered as the dashboard "System Health" tile and used
    /// by the Telegram alerter to notify on health-level transitions. Appended last so existing
    /// telemetry variant indices stay wire-stable.
    HealthSnapshot(crate::health::HealthSnapshot),
    /// A radio ENTERED active emergency — it sent an emergency status (U-STATUS, pre-coded status
    /// Emergency) to `dest_ssi`. Emitted once per session (on enter), not on the radio's periodic
    /// re-sends. Drives the dashboard emergency banner + Telegram alert. Appended last for
    /// bitcode wire-stability. (Emergency-priority CALLS are surfaced separately in the Active
    /// Calls table via the call's `priority`, not through this event.)
    EmergencyAlarm { source_issi: u32, dest_ssi: u32 },
    /// A radio's emergency was CLEARED (non-emergency status, clear-timeout, or operator clear).
    EmergencyCancel { source_issi: u32 },
    /// DAPNET message activity for the dashboard DAPNET tab. `direction` is "rx" for messages
    /// received from the RWTH core feed and "tx" for messages sent through the Hampager API.
    /// Appended last for bitcode wire-stability.
    DapnetLog {
        direction: String,
        id: String,
        callsign: String,
        recipient: String,
        text: String,
        priority: Option<u8>,
        paths: Vec<String>,
    },
    /// Multi-cell: the MS that just registered is on this cell (0 = primary). Sent right after
    /// its `MsRegistration`, only by stations running more than one cell. Appended last for
    /// bitcode wire-stability.
    MsCell { issi: u32, cell: u8 },
    /// Every cell the station runs (primary first), with its settings and live state — the same
    /// data as the dashboard Cells card. Emitted periodically. Appended last for bitcode
    /// wire-stability.
    CellsSnapshot { site_linked: bool, cells: Vec<CellInfo> },
    /// Multi-cell: an RF event (`TxVisual`, `TxQuality`, `SdrHealth`) from an additional cell's
    /// SDR, so the RF page can show every cell. The primary's are sent untagged, as before.
    /// Appended last for bitcode wire-stability.
    CellRf { cell: u8, event: CellRfEvent },
    /// The station's software version, sent to the telemetry server (e.g. the Brew server) every
    /// time the link connects so it can show which release each BTS runs. Appended last for
    /// bitcode wire-stability.
    StationVersion {
        /// Bost FlowStation release, e.g. "v0.5.1".
        version: String,
        /// Release plus build hash, e.g. "v0.5.1-5a76db7".
        build: String,
        /// Upstream FlowStation version the release is based on.
        upstream: String,
    },
    /// The station's configured position, for the telemetry server's map. Only sent when a
    /// non-zero latitude/longitude is configured (`[telemetry]`), periodically. Appended last for
    /// bitcode wire-stability.
    SiteLocation { name: Option<String>, lat: f64, lon: f64 },
    /// A radio's air-interface security state changed. `authenticated` is set once the radio
    /// passed a TAA1 challenge (EN 300 392-7 clause 4.4); `encrypting` once the MAC first
    /// decrypted a PDU it sent under the cell's static cipher key. `None` leaves that flag as it
    /// was. Appended last for bitcode wire-stability.
    MsSecurity { issi: u32, authenticated: Option<bool>, encrypting: Option<bool> },
    /// Progress of an over-the-air SCK delivery to a radio: "sent", "accepted", a failure
    /// text, or "timeout". Appended last for bitcode wire-stability.
    OtarSck { issi: u32, sckn: u8, sck_vn: u16, status: String },
    /// A radio left this cell for another cell or station without deregistering: it stays
    /// registered on the network, so this is not an `MsDeregistration`. `cell` is the cell it
    /// left (0 = primary), set by the cell-tagged sink like `MsCell`: a radio whose last `MsCell`
    /// still names that cell went to another station, not to a sibling cell. Appended last for
    /// bitcode wire-stability.
    MsMovedAway { issi: u32, cell: u8 },
}

impl TelemetryEvent {
    /// This build's [`TelemetryEvent::StationVersion`].
    pub fn station_version() -> Self {
        TelemetryEvent::StationVersion {
            version: tetra_core::PRODUCT_VERSION.to_string(),
            build: tetra_core::STACK_VERSION.to_string(),
            upstream: tetra_core::UPSTREAM_VERSION.to_string(),
        }
    }
}

/// The RF events an additional cell's SDR reports, mirroring the `TelemetryEvent` variants of the
/// same name (bitcode cannot encode a boxed `TelemetryEvent` inside itself).
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub enum CellRfEvent {
    TxVisual {
        sample_rate: f32,
        center_freq_hz: f64,
        rms_dbfs: f32,
        peak_dbfs: f32,
        spectrum_db_tenths: Vec<i16>,
        constellation_iq: Vec<i16>,
        carriers: Vec<(u16, f64)>,
        constellation_carrier: Option<(u16, f64)>,
    },
    TxQuality {
        papr_db: f32,
        evm_pct: f32,
        dc_offset_i: f32,
        dc_offset_q: f32,
        iq_amplitude_imbalance_db: f32,
        iq_phase_imbalance_deg: f32,
        carrier_leakage_db: f32,
        occupied_bandwidth_hz: f32,
        evm_carrier: Option<(u16, f64)>,
    },
    SdrHealth {
        temperature_c: Option<f32>,
        tx_gains: Vec<(String, f32)>,
        rx_gains: Vec<(String, f32)>,
    },
}

impl CellRfEvent {
    /// The RF part of `event`, or `event` back unchanged if it is not an RF event.
    pub fn from_event(event: TelemetryEvent) -> Result<Self, TelemetryEvent> {
        Ok(match event {
            TelemetryEvent::TxVisual {
                sample_rate,
                center_freq_hz,
                rms_dbfs,
                peak_dbfs,
                spectrum_db_tenths,
                constellation_iq,
                carriers,
                constellation_carrier,
            } => CellRfEvent::TxVisual {
                sample_rate,
                center_freq_hz,
                rms_dbfs,
                peak_dbfs,
                spectrum_db_tenths,
                constellation_iq,
                carriers,
                constellation_carrier,
            },
            TelemetryEvent::TxQuality {
                papr_db,
                evm_pct,
                dc_offset_i,
                dc_offset_q,
                iq_amplitude_imbalance_db,
                iq_phase_imbalance_deg,
                carrier_leakage_db,
                occupied_bandwidth_hz,
                evm_carrier,
            } => CellRfEvent::TxQuality {
                papr_db,
                evm_pct,
                dc_offset_i,
                dc_offset_q,
                iq_amplitude_imbalance_db,
                iq_phase_imbalance_deg,
                carrier_leakage_db,
                occupied_bandwidth_hz,
                evm_carrier,
            },
            TelemetryEvent::SdrHealth {
                temperature_c,
                tx_gains,
                rx_gains,
            } => CellRfEvent::SdrHealth {
                temperature_c,
                tx_gains,
                rx_gains,
            },
            other => return Err(other),
        })
    }

    /// The equivalent untagged `TelemetryEvent`.
    pub fn to_event(&self) -> TelemetryEvent {
        match self.clone() {
            CellRfEvent::TxVisual {
                sample_rate,
                center_freq_hz,
                rms_dbfs,
                peak_dbfs,
                spectrum_db_tenths,
                constellation_iq,
                carriers,
                constellation_carrier,
            } => TelemetryEvent::TxVisual {
                sample_rate,
                center_freq_hz,
                rms_dbfs,
                peak_dbfs,
                spectrum_db_tenths,
                constellation_iq,
                carriers,
                constellation_carrier,
            },
            CellRfEvent::TxQuality {
                papr_db,
                evm_pct,
                dc_offset_i,
                dc_offset_q,
                iq_amplitude_imbalance_db,
                iq_phase_imbalance_deg,
                carrier_leakage_db,
                occupied_bandwidth_hz,
                evm_carrier,
            } => TelemetryEvent::TxQuality {
                papr_db,
                evm_pct,
                dc_offset_i,
                dc_offset_q,
                iq_amplitude_imbalance_db,
                iq_phase_imbalance_deg,
                carrier_leakage_db,
                occupied_bandwidth_hz,
                evm_carrier,
            },
            CellRfEvent::SdrHealth {
                temperature_c,
                tx_gains,
                rx_gains,
            } => TelemetryEvent::SdrHealth {
                temperature_c,
                tx_gains,
                rx_gains,
            },
        }
    }
}

/// One cell's settings and live state, for [`TelemetryEvent::CellsSnapshot`].
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct CellInfo {
    pub id: u8,
    pub primary: bool,
    pub main_carrier: u16,
    pub secondary_carrier: Option<u16>,
    pub carriers: Vec<CellCarrierInfo>,
    /// Mobile country / network code this cell broadcasts (its network's, or `[net_info]`).
    pub mcc: u16,
    pub mnc: u16,
    /// The `[networks]` entry this cell serves ("default" without a `[networks]` section).
    pub network: String,
    /// The network's core link as seen from this cell: `Some(up)` with a Brew link, `None` for
    /// a network with no core.
    pub link_up: Option<bool>,
    pub colour_code: u8,
    pub location_area: u16,
    /// Neighbour cells advertised in D-NWRK-BROADCAST.
    pub neighbours: u16,
    /// SoapySDR device string, if configured.
    pub device: Option<String>,
    pub registered_radios: u32,
    /// ISSIs registered on this cell, sorted. Lets a consumer that missed registration events
    /// (e.g. after a telemetry reconnect) resync its list.
    pub registered_issis: Vec<u32>,
    /// "online" / "offline" / "error" / "starting"; None before the radio status is known.
    pub rf_state: Option<String>,
    pub rf_detail: Option<String>,
}

#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct CellCarrierInfo {
    pub carrier_num: u16,
    pub tx_freq_hz: u32,
    pub rx_freq_hz: u32,
}

/// A single host-system sensor reading. Kept flat for easy JSON serialisation
/// and rendering in tables.
#[derive(Debug, Clone, Encode, Decode, Serialize, Deserialize)]
pub struct SysSensor {
    /// Human label, e.g. "CPU package", "VDD_CORE", "Battery", "NVMe".
    pub name: String,
    /// What kind of measurement this is — drives the unit and the display column.
    pub kind: SysSensorKind,
    /// Numeric value in the unit implied by `kind`.
    pub value: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SysSensorKind {
    /// Degrees Celsius
    Temperature,
    /// Volts
    Voltage,
    /// Amperes
    Current,
    /// Watts (rail power = V × I, or RAPL energy/time)
    Power,
}
