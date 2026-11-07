use super::{ip, wap_ip, wap_status};
use crate::{MessageQueue, TetraEntityTrait};
use tetra_config::bluestation::SharedConfig;
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, Sap};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::ltpd::LtpdMleUnitdataInd;
use tetra_saps::tla::TlaTlDataReqBl;
use tetra_saps::{SapMsg, SapMsgInner};

// TETRA packet data (SNDCP). Air interface: ETSI EN 300 392-2 clause 28; PDU contents tables 28.23
// (SN-ACTIVATE PDP CONTEXT ACCEPT) and 28.24 (SN-ACTIVATE PDP CONTEXT DEMAND).

/// MLE protocol discriminator value for SNDCP (3 bits, 0b100). The MS's MLE routes the downlink SDU
/// to its SNDCP entity, mirroring the uplink (mle_bs.rs).
const MLE_DISCRIMINATOR_SNDCP: u64 = 0b100;
/// SN-PDU type for SN-ACTIVATE PDP CONTEXT (4 bits, 0x0). DEMAND on uplink, ACCEPT on downlink.
const SN_PDU_ACTIVATE_PDP_CONTEXT: u64 = 0x0;
// Remaining SN-PDU types handled by the WAP data path (clause 28.4.5, "SN PDU type").
const SN_PDU_DEACTIVATE_DEMAND: u8 = 2;
const SN_PDU_UNITDATA: u64 = 4;
const SN_PDU_DATA: u8 = 5;
const SN_PDU_DATA_TRANSMIT_REQUEST: u8 = 6;
const SN_PDU_DATA_TRANSMIT_RESPONSE: u64 = 7;
const SN_PDU_END_OF_DATA: u8 = 8;
const SN_PDU_RECONNECT: u8 = 9;

// WAP gateway endpoint defaults (spec §3). Overridable from `[cell_info.wap_ip]` once configured.
const WAP_DEFAULT_ADDRESS: [u8; 4] = [10, 0, 0, 1];
const WAP_DEFAULT_PORT: u16 = 9200;
const WAP_DEFAULT_RESPONSE_TTL: u8 = 32;
/// Status pages fetched over WSP are budgeted to the 545-octet SDU less the 4-byte WSP Reply
/// header; `serve_wsp_path` clamps further to the Openwave browser caps.
const WAP_WSP_STATUS_MAX_BYTES: usize = 541;

// PDCH timeslot policy (spec §5.3): a single SCCH/PDCH, releasing clears every slot. TS1 (the MCCH)
// is never assigned. The PDCH sits on TS4 because voice circuits are handed out first-fit from TS2
// upwards and always win a slot from packet data: on the last slot the two only meet once the cell
// is full. The MS follows whichever slot the allocation names.
const SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT: [bool; 4] = [false, false, false, true];
const SNDCP_PDCH_RELEASE_TIMESLOTS: [bool; 4] = [false, false, false, false];

// SN-ACTIVATE PDP CONTEXT ACCEPT mandatory field values (clause 28.4.5.*).
const PDU_PRIORITY_MAX: u64 = 4; // 0..7, mid (28.103)
const READY_TIMER: u64 = 8; // 8 = 10 s (28.112)
const STANDBY_TIMER: u64 = 5; // 5 = 10 min (28.122)
const RESPONSE_WAIT_TIMER: u64 = 8; // 8 = 10 s (28.116)
const TIA_IPV4_STATIC: u64 = 1; // Type identifier in accept: 1 = IPv4 Static Address (28.126)
const TIA_IPV4_DYNAMIC: u64 = 2; // 2 = IPv4 Dynamic Address
const MTU_1500: u64 = 4; // 4 = 1500 octets (28.79)
/// Address handed to a MS that requested a dynamic IPv4 (ATID != 0). Static requests get the address
/// the MS asked for, echoed back.
const POOL_IPV4: u64 = 0xC0A8_01B4; // 192.168.1.180

// CHAP authentication carried in the Protocol configuration options (PCO) type-3 element.
// Motorola/Dimetra radios run PPP CHAP (RFC 1994) inside PDP context activation: the DEMAND's PCO
// carries the MS's CHAP Response (username + MD5 hash). The SwMI must reply with a CHAP Success in
// the ACCEPT's PCO or the MS aborts the data session ("data server not responding").
const PCO_TYPE34_ID: u64 = 1; // Table 28.127: type-3/4 element identifier for PCO
const PPP_PROTO_CHAP: u64 = 0xC223; // RFC 3232 configuration protocol identifier for CHAP
const PPP_CONFIG_PROTOCOL_PPP: u64 = 0; // Table 28.105: configuration protocol 0000 = PPP
const CHAP_CODE_SUCCESS: u64 = 3; // RFC 1994 CHAP code: 3 = Success
/// PCO content length, in bits, for a bare CHAP Success: configuration protocol(4) +
/// protocol identity(16) + length-of-contents(8) + CHAP Success packet(4 octets = 32) = 60.
const PCO_CHAP_SUCCESS_BITS: u64 = 60;

pub struct Sndcp {
    #[allow(dead_code)] // wired up when the full packet-data state machine is implemented
    config: SharedConfig,
}

impl Sndcp {
    pub fn new(config: SharedConfig) -> Self {
        Self { config }
    }

    /// Reply to an SN-ACTIVATE PDP CONTEXT DEMAND with a spec-conformant SN-ACTIVATE PDP CONTEXT
    /// ACCEPT (table 28.23): echo the requested NSAPI and grant the IPv4 the MS asked for (or one
    /// from the pool for a dynamic request), with mandatory timers/MTU and no optional elements.
    /// `demand` is the SNDCP PDU bit-string (after the 3-bit MLE discriminator).
    fn send_pdp_accept(&self, queue: &mut MessageQueue, ind: &LtpdMleUnitdataInd, demand: &str) {
        // Decode the DEMAND mandatory header (table 28.24): type(4) version(4) NSAPI(4) ATID(3)
        // [IPv4(32) when ATID==0].
        let bits = |off: usize, n: usize| -> Option<u64> { demand.get(off..off + n).and_then(|s| u64::from_str_radix(s, 2).ok()) };
        let nsapi = bits(8, 4).unwrap_or(1);
        let atid = bits(12, 3).unwrap_or(0);
        let (tia, ipv4) = if atid == 0 {
            // Static: the MS asked for a specific IPv4 (bits 15..47) — grant it.
            (TIA_IPV4_STATIC, bits(15, 32).unwrap_or(POOL_IPV4))
        } else {
            // Dynamic (or other): assign one from the pool.
            (TIA_IPV4_DYNAMIC, POOL_IPV4)
        };

        // Build the ACCEPT (table 28.23), prefixed with the 3-bit MLE SNDCP discriminator.
        let mut sdu = BitBuffer::new_autoexpand(16);
        sdu.write_bits(MLE_DISCRIMINATOR_SNDCP, 3);
        sdu.write_bits(SN_PDU_ACTIVATE_PDP_CONTEXT, 4); // SN PDU type = ACCEPT
        sdu.write_bits(nsapi, 4); // NSAPI (echo)
        sdu.write_bits(PDU_PRIORITY_MAX, 3);
        sdu.write_bits(READY_TIMER, 4);
        sdu.write_bits(STANDBY_TIMER, 4);
        sdu.write_bits(RESPONSE_WAIT_TIMER, 4);
        sdu.write_bits(tia, 3); // Type identifier in accept (IPv4 present)
        sdu.write_bits(ipv4, 32); // IP Address IPv4 (conditional on TIA = 1/2)
        sdu.write_bits(0, 8); // PCOMP negotiation = 0 (no header compression → no conditional fields)
        sdu.write_bits(MTU_1500, 3); // Maximum transmission unit

        // Optional elements (annex E.1). If the DEMAND carried PPP CHAP authentication the MS is
        // waiting for a CHAP Success — without it the data session never opens ("data server not
        // responding"). Append a PCO type-3 element carrying the Success; otherwise close the PDU
        // with an o-bit of 0.
        let chap_id = find_chap_response_id(demand);
        if let Some(id) = chap_id {
            for bit in chap_success_optional_section(id).bytes() {
                sdu.write_bits(u64::from(bit - b'0'), 1);
            }
        } else {
            sdu.write_bits(0, 1); // o-bit = 0: no optional elements → PDU ends
        }
        let len = sdu.get_pos();
        sdu.seek(0);

        tracing::info!(
            "SNDCP: -> SN-ACTIVATE PDP CONTEXT ACCEPT to {:?}: NSAPI={} TIA={} IPv4={}.{}.{}.{} CHAP-Success={} ({} bits)",
            ind.received_tetra_address,
            nsapi,
            tia,
            (ipv4 >> 24) & 0xff,
            (ipv4 >> 16) & 0xff,
            (ipv4 >> 8) & 0xff,
            ipv4 & 0xff,
            chap_id.map(|id| format!("id={id}")).unwrap_or_else(|| "none".into()),
            len
        );

        // Acknowledged basic link (the DEMAND arrived acknowledged), addressed to the requesting MS.
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: ind.received_tetra_address,
                link_id: ind.link_id,
                endpoint_id: ind.endpoint_id,
                tl_sdu: sdu,
                stealing_permission: false,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0,
                graceful_degradation: None,
                chan_alloc: None,
                stealing_slot: None,
                tx_reporter: None,
            }),
        });
    }

    /// The WAP gateway is active only when the cell advertises SNDCP packet-data service *and* the
    /// WAP-IP profile is enabled. Config validation keeps these two in lock-step (spec §6.1), so
    /// this single predicate is authoritative.
    fn wap_enabled(&self) -> bool {
        // Single source of truth: the `[cell_info.wap_ip].enabled` master switch.
        self.config.config().cell.wap_enabled()
    }

    /// The BS WAP endpoint responses originate from (source address/port and the TTL to send with),
    /// read from `[cell_info.wap_ip]` or falling back to the spec defaults.
    fn wap_endpoint(&self) -> wap_ip::WapEndpoint {
        match self.config.config().cell.wap_ip {
            Some(w) => wap_ip::WapEndpoint { address: w.address, port: w.port, response_ttl: w.response_ttl },
            None => wap_ip::WapEndpoint {
                address: WAP_DEFAULT_ADDRESS,
                port: WAP_DEFAULT_PORT,
                response_ttl: WAP_DEFAULT_RESPONSE_TTL,
            },
        }
    }

    /// Build the WAP status snapshot from process health counters and the attached-radio directory.
    /// Both are fed by telemetry even when no dashboard or telemetry server is configured.
    fn build_snapshot(&self) -> wap_status::WapStatusSnapshot {
        let reg = crate::health::registry();
        let health = match reg.overall_level() {
            crate::health::HealthLevel::Ok => "OK",
            crate::health::HealthLevel::Degraded => "DEGRADED",
            crate::health::HealthLevel::Critical => "CRITICAL",
        };
        let last_activity = match reg.last_radio_activity_age_secs() {
            Some(age) => format!("{} ago", wap_status::compact_tiny_uptime(age)),
            None => "none".to_string(),
        };
        // Per-radio detail and the group/private split come from the always-on radio directory
        // (fed by the telemetry fan-out, independent of the dashboard). Capped so the lines never
        // blow the WAP page budget; the renderer's size ladder trims further if needed.
        let radio_lines = crate::radio_directory::lines(16).iter().map(Self::format_radio_line).collect();
        wap_status::WapStatusSnapshot {
            title: "FlowStation".to_string(),
            version: tetra_core::PRODUCT_VERSION.to_string(),
            state: "ON AIR".to_string(),
            registered_ms: reg.registered_radios() as u32,
            group_count: crate::radio_directory::group_affiliation_count() as u32,
            private_count: crate::radio_directory::private_count() as u32,
            sds_count: reg.sds_seen().min(u32::MAX as u64) as u32,
            radio_lines,
            uptime_secs: reg.uptime_secs(),
            last_activity,
            health: health.to_string(),
        }
    }

    /// Format one radio-directory row as a compact status line for the WAP page, e.g.
    /// `1001 -85dBFS 0d0h0m3s` (idle time), or `1002 -- 0d0h1m0s` when no signal reading is held.
    fn format_radio_line(r: &crate::radio_directory::RadioLine) -> String {
        let rssi = match r.rssi_dbfs {
            Some(v) => format!("{}dBFS", v.round() as i32),
            None => "--".to_string(),
        };
        format!("{} {} {}", r.issi, rssi, wap_status::compact_tiny_uptime(r.idle_secs))
    }

    /// Handle an uplink SN-UNITDATA (type 4) or SN-DATA (type 1/5) carrying an IPv4/UDP N-PDU: parse
    /// it, classify the WAP request, render the response, and send it back as a downlink SN-UNITDATA
    /// (always type 4). `pdu` is the SN-PDU bit-string (after the 3-bit MLE discriminator).
    fn handle_wap_data(&self, queue: &mut MessageQueue, ind: &LtpdMleUnitdataInd, pdu: &str) {
        if !self.wap_enabled() {
            tracing::debug!("SNDCP: WAP-IP profile disabled; dropping SN-DATA/UNITDATA");
            return;
        }
        // SN-UNITDATA/SN-DATA header (16 bits): type(4) NSAPI(4) PCOMP(4) DCOMP(4).
        let bits = |off: usize, n: usize| -> Option<u64> { pdu.get(off..off + n).and_then(|s| u64::from_str_radix(s, 2).ok()) };
        let nsapi = bits(4, 4).unwrap_or(1);
        let (pcomp, dcomp) = (bits(8, 4).unwrap_or(0), bits(12, 4).unwrap_or(0));
        if pcomp != 0 || dcomp != 0 {
            tracing::info!("SNDCP: unsupported N-PDU compression (pcomp={pcomp} dcomp={dcomp}); dropping");
            return;
        }
        let Some(npdu) = pdu.get(16..).and_then(npdu_bits_to_bytes) else {
            tracing::info!("SNDCP: N-PDU empty or not octet-aligned; dropping");
            return;
        };

        let endpoint = self.wap_endpoint();
        let snapshot = self.build_snapshot();
        let Some(datagram) = build_wap_response(&endpoint, &npdu, &snapshot) else {
            return; // not UDP, not for the gateway port, a WTP control PDU, or unsupported
        };

        tracing::info!(
            "SNDCP: -> WAP response to {:?}: NSAPI={} {} datagram bytes",
            ind.received_tetra_address,
            nsapi,
            datagram.len(),
        );
        let sdu = build_downlink_unitdata_sdu(nsapi, &datagram);
        self.send_sndcp_downlink(queue, ind, sdu, None);
    }

    /// Grant (or release) the packet-data channel: emit an SN-DATA TRANSMIT RESPONSE and attach the
    /// PDCH channel allocation. A new/refresh grant assigns the PDCH slot; an end-of-data release quits the
    /// channel back to common control (spec §5.3).
    fn send_pdch_grant(&self, queue: &mut MessageQueue, ind: &LtpdMleUnitdataInd, alloc: CmceChanAllocReq, label: &str) {
        if !self.wap_enabled() {
            return;
        }
        let mut sdu = BitBuffer::new_autoexpand(2);
        sdu.write_bits(MLE_DISCRIMINATOR_SNDCP, 3);
        sdu.write_bits(SN_PDU_DATA_TRANSMIT_RESPONSE, 4);
        sdu.write_bits(0, 1); // result = accepted
        sdu.seek(0);
        tracing::info!(
            "SNDCP: -> SN-DATA TRANSMIT RESPONSE ({label}) to {:?}: timeslots={:?} {:?}/{:?}",
            ind.received_tetra_address,
            alloc.timeslots,
            alloc.alloc_type,
            alloc.ul_dl_assigned,
        );
        self.send_sndcp_downlink(queue, ind, sdu, Some(alloc));
    }

    /// Push a downlink SNDCP PDU (3-bit discriminator already prepended) to LLC over the acknowledged
    /// basic link, optionally carrying a PDCH channel allocation.
    fn send_sndcp_downlink(&self, queue: &mut MessageQueue, ind: &LtpdMleUnitdataInd, sdu: BitBuffer, chan_alloc: Option<CmceChanAllocReq>) {
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqBl(TlaTlDataReqBl {
                main_address: ind.received_tetra_address,
                link_id: ind.link_id,
                endpoint_id: ind.endpoint_id,
                tl_sdu: sdu,
                stealing_permission: false,
                subscriber_class: 0,
                fcs_flag: false,
                air_interface_encryption: None,
                stealing_repeats_flag: None,
                data_class_info: None,
                req_handle: 0,
                graceful_degradation: None,
                chan_alloc,
                stealing_slot: None,
                tx_reporter: None,
            }),
        });
    }
}

/// The WAP gateway core: validate and parse an uplink N-PDU, classify the WAP request, render the
/// response, and return the downlink IPv4/UDP datagram — or `None` when the request must be dropped
/// (not UDP, not addressed to the gateway port, a WTP control PDU, or unsupported). Pure and
/// side-effect-free so the conformance vectors can exercise it directly.
fn build_wap_response(endpoint: &wap_ip::WapEndpoint, npdu: &[u8], snapshot: &wap_status::WapStatusSnapshot) -> Option<Vec<u8>> {
    let ipv4 = ip::parse_ipv4(npdu).ok()?;
    if ipv4.protocol != ip::IPV4_PROTOCOL_UDP {
        return None;
    }
    let udp = ip::parse_udp(ipv4.payload).ok()?;
    if udp.dst_port != endpoint.port {
        return None;
    }
    let udp_payload = match wap_ip::classify(udp.payload) {
        wap_ip::WapRequest::WtpWspConnect { tid, client_sdu, server_sdu, .. } => {
            wap_ip::wtp_result(tid, &wap_ip::wsp_connect_reply(client_sdu, server_sdu))
        }
        wap_ip::WapRequest::WtpWspGet { tid, path, .. } => {
            let (content_type, body) = wap_status::serve_wsp_path(&path, snapshot, WAP_WSP_STATUS_MAX_BYTES);
            wap_ip::wtp_result(tid, &wap_ip::wsp_reply(content_type, &body))
        }
        wap_ip::WapRequest::WtpWspResume { tid, .. } => wap_ip::wtp_result(tid, &wap_ip::wsp_resume_ok()),
        wap_ip::WapRequest::TextGet { .. } => {
            // Raw-UDP path: the rendered page is the UDP payload directly (no WTP/WSP framing).
            wap_status::render_wml2_status(snapshot, wap_status::DEFAULT_WAP_STATUS_MAX_BYTES).ok()?
        }
        wap_ip::WapRequest::WtpControlNoResponse { .. } | wap_ip::WapRequest::Unsupported { .. } => return None,
    };
    Some(wap_ip::build_response_datagram(endpoint, &ipv4, udp.src_port, ipv4.id, &udp_payload))
}

/// Convert a bit-string of an N-PDU to octets. Returns `None` for an empty or non-octet-aligned
/// N-PDU (an IP datagram is always a whole number of bytes).
fn npdu_bits_to_bytes(bits: &str) -> Option<Vec<u8>> {
    if bits.is_empty() || bits.len() % 8 != 0 {
        return None;
    }
    (0..bits.len()).step_by(8).map(|i| u8::from_str_radix(&bits[i..i + 8], 2).ok()).collect()
}

/// Encapsulate a downlink IPv4/UDP datagram in an SN-UNITDATA PDU (always type 4, even for an
/// SN-DATA request), prefixed with the 3-bit MLE SNDCP discriminator. Header: type(4) NSAPI(4)
/// PCOMP(4)=0 DCOMP(4)=0, then the datagram copied bit-for-bit (spec §5.1).
fn build_downlink_unitdata_sdu(nsapi: u64, datagram: &[u8]) -> BitBuffer {
    let mut sdu = BitBuffer::new_autoexpand(3 + datagram.len());
    sdu.write_bits(MLE_DISCRIMINATOR_SNDCP, 3);
    sdu.write_bits(SN_PDU_UNITDATA, 4);
    sdu.write_bits(nsapi, 4);
    sdu.write_bits(0, 4); // PCOMP = 0
    sdu.write_bits(0, 4); // DCOMP = 0
    for &byte in datagram {
        sdu.write_bits(u64::from(byte), 8);
    }
    sdu.seek(0);
    sdu
}

/// Collapse any multi-slot timeslot policy to a single slot, preferring the first set slot beyond
/// TS1 (the MCCH is never assigned to a PDCH) and only then falling back to TS1 (spec §5.3).
fn normalize_pdch_timeslots_to_single(ts: [bool; 4]) -> [bool; 4] {
    let mut out = [false; 4];
    if let Some(i) = (1..4).find(|&i| ts[i]).or_else(|| (0..4).find(|&i| ts[i])) {
        out[i] = true;
    }
    out
}

/// The timeslots assigned for a resource request. The MS's own resource request is ignored; the BS
/// always assigns the single SCCH/PDCH timeslot (spec §5.3).
fn assigned_scch_pdch_timeslots_for_resource_request() -> [bool; 4] {
    SNDCP_PDCH_SINGLE_ASSIGNED_SCCH_TIMESLOT
}

/// Channel allocation for an SN-DATA TRANSMIT RESPONSE: assign the PDCH slot, replacing any
/// existing allocation, bidirectional (spec §5.3).
fn pdch_allocation_for_data_transmit_response() -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: None,
        carrier: None,
        timeslots: normalize_pdch_timeslots_to_single(assigned_scch_pdch_timeslots_for_resource_request()),
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
        cell_change: false,
    }
}

/// Channel allocation for SN-END OF DATA: release the PDCH and return the MS to common control
/// (QuitAndGo, no timeslots) (spec §5.3).
fn pdch_allocation_for_end_of_data() -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: None,
        carrier: None,
        timeslots: SNDCP_PDCH_RELEASE_TIMESLOTS,
        alloc_type: ChanAllocType::QuitAndGo,
        ul_dl_assigned: UlDlAssignment::Both,
        cell_change: false,
    }
}

/// Build the optional-element section (annex E.1) of an ACCEPT that grants a CHAP Success with the
/// given identifier: the o-bit, the three absent type-2 presence bits (SNDCP network endpoint id,
/// SwMI IPv6 information, SwMI Mobile IPv4 information — table 28.23), the PCO type-3 element, and
/// the closing m-bit. Returned MSB-first as a bit string for direct append to the PDU. Kept as the
/// single source of truth so the wire encoding and its unit test cannot drift apart.
fn chap_success_optional_section(chap_id: u8) -> String {
    let mut s = String::with_capacity(81);
    s.push('1'); // o-bit = 1: optional elements follow
    s.push_str("000"); // type-2 presence bits, in table order, all absent
    s.push('1'); // M-bit = 1: a type-3/4 element follows
    s.push_str(&format!("{PCO_TYPE34_ID:04b}")); // type-3/4 element identifier (PCO = 1)
    s.push_str(&format!("{PCO_CHAP_SUCCESS_BITS:011b}")); // length indicator (bits)
    // PCO content (table 28.105): PPP / CHAP / one Success packet.
    s.push_str(&format!("{PPP_CONFIG_PROTOCOL_PPP:04b}")); // configuration protocol = PPP
    s.push_str(&format!("{PPP_PROTO_CHAP:016b}")); // protocol identity = CHAP (C223H)
    s.push_str(&format!("{:08b}", 4)); // length of protocol identity contents = 4 octets
    // CHAP Success packet (RFC 1994): code=3, identifier echoed from the MS Response, length=4
    // (header only, no message).
    s.push_str(&format!("{CHAP_CODE_SUCCESS:08b}"));
    s.push_str(&format!("{chap_id:08b}"));
    s.push_str(&format!("{:016b}", 4));
    s.push('0'); // M-bit = 0: no more type-3/4 elements (last bit in the PDU)
    s
}

/// Scan an SN-ACTIVATE PDP CONTEXT DEMAND bit-string for a PPP CHAP packet carried in its Protocol
/// configuration options element and return the identifier of the MS's CHAP Response (RFC 1994 code
/// 2), to be echoed in the Success we send back. Falls back to a Challenge's (code 1) identifier if
/// no Response is present, or `None` if the DEMAND carries no CHAP at all (e.g. PAP or no auth → we
/// leave the ACCEPT without a PCO).
///
/// The scan locates the 16-bit CHAP configuration-protocol identifier (C223H) at any bit offset —
/// the PCO is bit-packed, not byte-aligned — then reads the CHAP packet's code/identifier from the
/// fixed offsets that follow (8-bit length-of-contents, then code, then identifier). The CHAP code
/// is validated to reject a coincidental C223H bit pattern inside a hash value.
fn find_chap_response_id(demand: &str) -> Option<u8> {
    const CHAP_PROTO_ID: &str = "1100001000100011"; // C223H, MSB first
    let read = |off: usize| -> Option<u8> { demand.get(off..off + 8).and_then(|s| u8::from_str_radix(s, 2).ok()) };
    let mut fallback = None;
    let mut from = 0;
    while let Some(rel) = demand.get(from..).and_then(|s| s.find(CHAP_PROTO_ID)) {
        let marker = from + rel;
        // After C223H: length-of-contents (8 bits), then the CHAP packet (code, identifier, ...).
        match (read(marker + 16 + 8), read(marker + 16 + 16)) {
            (Some(2), Some(id)) => return Some(id),                           // Response — echo this identifier
            (Some(1), Some(id)) if fallback.is_none() => fallback = Some(id), // Challenge
            _ => {}
        }
        from = marker + CHAP_PROTO_ID.len();
    }
    fallback
}

impl TetraEntityTrait for Sndcp {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Sndcp
    }

    fn rx_prim(&mut self, queue: &mut MessageQueue, message: SapMsg) {
        // Decode the SN-PDU type and, for an SN-ACTIVATE PDP CONTEXT DEMAND, return an ACCEPT.
        // Never panics: a garbled PDU must not take down the stack.
        let SapMsgInner::LtpdMleUnitdataInd(ind) = &message.msg else {
            tracing::debug!("SNDCP: unhandled prim (sap={:?}): {:?}", message.sap, message.msg);
            return;
        };

        // The SDU still carries the leading 3-bit MLE protocol discriminator (0b100 = SNDCP); the
        // SNDCP PDU proper — which begins with a 4-bit SN-PDU type — starts after it.
        let raw = ind.sdu.dump_bin_unformatted();
        let pdu = raw.get(3..).unwrap_or("");
        let sn_type = pdu.get(0..4).and_then(|s| u8::from_str_radix(s, 2).ok());
        // SN-PDU type table (ETSI EN 300 392-2 clause 28.4.5, "SN PDU type").
        let name = match sn_type {
            Some(0) => "SN-ACTIVATE PDP CONTEXT (DEMAND/ACCEPT)",
            Some(1) => "SN-DEACTIVATE PDP CONTEXT ACCEPT",
            Some(2) => "SN-DEACTIVATE PDP CONTEXT DEMAND",
            Some(3) => "SN-ACTIVATE PDP CONTEXT REJECT",
            Some(4) => "SN-UNITDATA",
            Some(5) => "SN-DATA",
            Some(6) => "SN-DATA TRANSMIT REQUEST",
            Some(7) => "SN-DATA TRANSMIT RESPONSE",
            Some(8) => "SN-END OF DATA",
            Some(9) => "SN-RECONNECT",
            Some(10) => "SN-PAGE REQUEST/RESPONSE",
            Some(11) => "SN-NOT SUPPORTED",
            Some(12) => "SN-DATA PRIORITY",
            Some(13) => "SN-MODIFY",
            _ => "reserved/unknown",
        };
        tracing::info!(
            "SNDCP: <- packet-data PDU from {:?} — SN-PDU type 0x{} ({}), {} bits",
            ind.received_tetra_address,
            sn_type.map(|n| format!("{n:x}")).unwrap_or_else(|| "?".into()),
            name,
            pdu.len(),
        );

        match sn_type {
            Some(t) if t == SN_PDU_ACTIVATE_PDP_CONTEXT as u8 => self.send_pdp_accept(queue, ind, pdu),
            Some(SN_PDU_DATA_TRANSMIT_REQUEST) => {
                self.send_pdch_grant(queue, ind, pdch_allocation_for_data_transmit_response(), "assign PDCH");
            }
            Some(SN_PDU_RECONNECT) => {
                // The MS changed cell with data outstanding and asks to carry on here (clause
                // 28.3.4). Contexts are not tied to a cell, so the answer is the same as for a
                // fresh transmit request: accept and assign this cell's PDCH.
                self.send_pdch_grant(queue, ind, pdch_allocation_for_data_transmit_response(), "reconnect, assign PDCH");
            }
            Some(SN_PDU_END_OF_DATA) => {
                self.send_pdch_grant(queue, ind, pdch_allocation_for_end_of_data(), "release PDCH");
            }
            Some(t) if t == SN_PDU_UNITDATA as u8 || t == SN_PDU_DATA => self.handle_wap_data(queue, ind, pdu),
            Some(SN_PDU_DEACTIVATE_DEMAND) => {
                // The MS asked to tear the context down. We let it lapse via the standby timer rather
                // than emit a DEACTIVATE ACCEPT, whose exact on-air layout is not pinned by a vector.
                tracing::info!("SNDCP: <- SN-DEACTIVATE PDP CONTEXT DEMAND from {:?}; context will lapse on standby timeout", ind.received_tetra_address);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_to_bits(hex: &str) -> String {
        hex.chars().map(|c| format!("{:04b}", c.to_digit(16).unwrap())).collect()
    }

    #[test]
    fn finds_chap_response_identifier_in_real_demand_pco() {
        // Captured SN-ACTIVATE PDP CONTEXT DEMAND PCO content from a Motorola radio: a CHAP
        // Challenge (id 5, name "DIMETRA…") followed by a CHAP Response (id 5, username "admin").
        let pco = hex_to_bits(
            "0c22318010500180aac20e0caf974bc75e02f44494d455452415f50\
             c2231a0205001a10db3b2df8c57cce0db8712b16aa9cb5a361646d696",
        );
        assert_eq!(find_chap_response_id(&pco), Some(5));
    }

    #[test]
    fn prefers_response_over_challenge_and_skips_non_chap_bits() {
        let mut s = String::from("101"); // leading bits that are not part of any C223H marker
        s.push_str("1100001000100011"); // C223H
        s.push_str("00000110"); // length-of-contents (ignored by the scan)
        s.push_str("00000001"); // CHAP code = 1 (Challenge)
        s.push_str("00001001"); // identifier = 9
        s.push_str("1100001000100011"); // C223H
        s.push_str("00000110"); // length-of-contents
        s.push_str("00000010"); // CHAP code = 2 (Response)
        s.push_str("00000111"); // identifier = 7
        assert_eq!(find_chap_response_id(&s), Some(7));
    }

    #[test]
    fn no_chap_in_demand_returns_none() {
        assert_eq!(find_chap_response_id(&"0".repeat(256)), None);
    }

    #[test]
    fn radio_line_preserves_the_measurement_unit() {
        let row = crate::radio_directory::RadioLine { issi: 1001, rssi_dbfs: Some(-23.4), idle_secs: 3, group_count: 1 };
        assert_eq!(Sndcp::format_radio_line(&row), "1001 -23dBFS 0d0h0m3s");
        let unknown = crate::radio_directory::RadioLine { issi: 1002, rssi_dbfs: None, idle_secs: 3, group_count: 0 };
        assert_eq!(Sndcp::format_radio_line(&unknown), "1002 -- 0d0h0m3s");
    }

    fn test_endpoint() -> wap_ip::WapEndpoint {
        wap_ip::WapEndpoint { address: WAP_DEFAULT_ADDRESS, port: WAP_DEFAULT_PORT, response_ttl: WAP_DEFAULT_RESPONSE_TTL }
    }

    fn test_snapshot() -> wap_status::WapStatusSnapshot {
        wap_status::WapStatusSnapshot { title: "FlowStation".into(), version: "v0.5.4".into(), state: "ON AIR".into(), health: "OK".into(), ..Default::default() }
    }

    // ---- §5.3 PDCH channel allocation ----

    #[test]
    fn pdch_grant_assigns_single_slot_clear_of_voice() {
        let a = pdch_allocation_for_data_transmit_response();
        assert_eq!(a.timeslots, [false, false, false, true]); // TS4 only: the last slot voice takes
        assert_eq!(a.alloc_type, ChanAllocType::Replace);
        assert_eq!(a.ul_dl_assigned, UlDlAssignment::Both);
        assert!(a.usage.is_none() && a.carrier.is_none());
    }

    #[test]
    fn pdch_release_quits_and_clears_slots() {
        let a = pdch_allocation_for_end_of_data();
        assert_eq!(a.timeslots, [false, false, false, false]);
        assert_eq!(a.alloc_type, ChanAllocType::QuitAndGo);
        assert_eq!(a.ul_dl_assigned, UlDlAssignment::Both);
    }

    #[test]
    fn normalize_collapses_multislot_preferring_beyond_ts1() {
        assert_eq!(normalize_pdch_timeslots_to_single([true, true, false, true]), [false, true, false, false]);
        assert_eq!(normalize_pdch_timeslots_to_single([false, false, true, true]), [false, false, true, false]);
        assert_eq!(normalize_pdch_timeslots_to_single([true, false, false, false]), [true, false, false, false]); // only TS1 set
        assert_eq!(normalize_pdch_timeslots_to_single([false, false, false, false]), [false, false, false, false]);
    }

    // ---- N-PDU octet conversion ----

    #[test]
    fn npdu_bits_to_bytes_vectors() {
        assert_eq!(npdu_bits_to_bytes("0100010100000000"), Some(vec![0x45, 0x00]));
        assert_eq!(npdu_bits_to_bytes("010"), None); // not octet-aligned
        assert_eq!(npdu_bits_to_bytes(""), None); // empty
    }

    // ---- downlink SN-UNITDATA framing (§5.1) ----

    #[test]
    fn downlink_unitdata_sdu_frames_type4_with_discriminator() {
        let sdu = build_downlink_unitdata_sdu(2, &[0x45, 0x00]);
        let bits = sdu.dump_bin_unformatted();
        assert!(bits.starts_with("100"), "3-bit MLE SNDCP discriminator"); // 0b100
        assert_eq!(&bits[3..7], "0100", "SN-PDU type = 4 (UNITDATA)");
        assert_eq!(&bits[7..11], "0010", "NSAPI echoed = 2");
        assert_eq!(&bits[11..15], "0000", "PCOMP = 0");
        assert_eq!(&bits[15..19], "0000", "DCOMP = 0");
        assert_eq!(&bits[19..27], "01000101", "first N-PDU octet 0x45");
    }

    // ---- WAP gateway core (§5) ----

    #[test]
    fn wap_response_renders_text_get_and_swaps_endpoints() {
        // Uplink IPv4/UDP from the MS (10.0.0.226:49152) to the gateway (10.0.0.1:9200).
        let npdu = ip::build_ipv4_udp([10, 0, 0, 226], [10, 0, 0, 1], 49152, 9200, 7, 32, b"GET /status.xhtml");
        let datagram = build_wap_response(&test_endpoint(), &npdu, &test_snapshot()).expect("rendered");

        let resp = ip::parse_ipv4(&datagram).unwrap();
        assert_eq!(resp.src, [10, 0, 0, 1], "response source = gateway");
        assert_eq!(resp.dst, [10, 0, 0, 226], "response destination = MS");
        assert_eq!(resp.id, 8, "IP id = request id + 1");
        let udp = ip::parse_udp(resp.payload).unwrap();
        assert_eq!(udp.src_port, 9200);
        assert_eq!(udp.dst_port, 49152, "ports swapped");
        let body = String::from_utf8_lossy(udp.payload);
        assert!(body.contains("FlowStation"), "rendered status page");
    }

    #[test]
    fn wap_response_drops_wrong_port() {
        let npdu = ip::build_ipv4_udp([10, 0, 0, 226], [10, 0, 0, 1], 49152, 8080, 7, 32, b"GET /");
        assert!(build_wap_response(&test_endpoint(), &npdu, &test_snapshot()).is_none());
    }

    #[test]
    fn wap_response_drops_non_udp() {
        let npdu = ip::build_ipv4_tcp([10, 0, 0, 226], [10, 0, 0, 1], 49152, 9200, 0, 0, 0x002, 0x1000, 7, 32, b"");
        assert!(build_wap_response(&test_endpoint(), &npdu, &test_snapshot()).is_none());
    }

    #[test]
    fn wap_response_drops_garbage_npdu() {
        assert!(build_wap_response(&test_endpoint(), &[0xff; 4], &test_snapshot()).is_none());
    }

    #[test]
    fn optional_section_layout_matches_spec() {
        let sec = chap_success_optional_section(5);
        // o-bit(1) + type-2 P-bits(3) + M-bit(1) + id(4) + length(11) + PCO(60) + M-bit(1) = 81.
        assert_eq!(sec.len(), 81);
        assert_eq!(&sec[0..4], "1000"); // o-bit=1, three absent type-2 presence bits
        assert_eq!(&sec[4..5], "1"); // M-bit = 1
        assert_eq!(&sec[5..9], "0001"); // type-3/4 element identifier = 1 (PCO)
        assert_eq!(&sec[9..20], &format!("{PCO_CHAP_SUCCESS_BITS:011b}")); // length indicator
        assert_eq!(&sec[20..24], "0000"); // configuration protocol = PPP
        assert_eq!(&sec[24..40], "1100001000100011"); // protocol identity = CHAP
        assert_eq!(&sec[40..48], "00000100"); // length of contents = 4 octets
        assert_eq!(&sec[48..56], "00000011"); // CHAP code = 3 (Success)
        assert_eq!(&sec[56..64], "00000101"); // identifier echoed = 5
        assert_eq!(&sec[64..80], "0000000000000100"); // CHAP length = 4
        assert_eq!(&sec[80..81], "0"); // closing M-bit = 0
    }
}
