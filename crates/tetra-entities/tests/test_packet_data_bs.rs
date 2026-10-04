//! `[packet_data]`: the SNDCP bearer between the radios and the WAP gateway, tested with the real
//! LLC, MLE and SNDCP entities and a simulated radio on the other side of the MAC.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::net::Ipv4Addr;

use common::ComponentTest;
use tetra_config::bluestation::{PacketDataBearer, PdchGrant, SharedConfig, StackConfig, StackMode};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, CarrierSlot, Sap, SsiType, TdmaTime, TetraAddress, TimeslotOwner, TxReporter, TxState, debug};
use tetra_entities::llc::components::fcs;
use tetra_entities::sndcp::ip::{build_ipv4_udp_npdu, parse_ipv4_packet, parse_udp_datagram};
use tetra_entities::sndcp::transfer::{
    SndcpDataTransmitRequest, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpPacketDataResourceRequest,
    SndcpPhaseModulationResourceRequest, SndcpReconnect, SndcpTransferRejectCause, decode_data_transmit_response, decode_end_of_data,
    decode_not_supported, encode_data_transmit_request, encode_end_of_data, encode_reconnect,
};
use tetra_entities::sndcp::unitdata::{decode_sn_user_data_pdu, encode_sn_data, encode_sn_unitdata};
use tetra_pdus::llc::enums::llc_pdu_type::LlcPduType;
use tetra_pdus::llc::pdus::al_ack::AlAck;
use tetra_pdus::llc::pdus::al_data::AlData;
use tetra_pdus::llc::pdus::al_setup::AlSetup;
use tetra_pdus::llc::pdus::{bl_ack::BlAck, bl_adata::BlAdata, bl_data::BlData, bl_udata::BlUdata};
use tetra_pdus::mle::enums::mle_protocol_discriminator::MleProtocolDiscriminator;
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::ltpd::LtpdBearer;
use tetra_saps::sapmsg::{SapMsg, SapMsgInner};
use tetra_saps::tla::{TlDataIndAl, TlaTlDataIndBl, TlaTlUnitdataIndBl};
use tetra_saps::tma::{DATA_CATEGORY_PDCH_ASSIGNMENT, TmaUnitdataInd, TmaUnitdataReq};

const MAIN_CARRIER: u16 = 1521;
const ISSI: u32 = 2_260_618;
const ISSI2: u32 = 2_260_619;
const GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 0, 0, 1);
const RADIO_PORT: u16 = 2049;
/// Captured PCO content of a Motorola DEMAND: CHAP Challenge and CHAP Response, both id 5.
const REAL_PCO_HEX: &str =
    "0c22318010500180aac20e0caf974bc75e02f44494d455452415f50c2231a0205001a10db3b2df8c57cce0db8712b16aa9cb5a361646d696";

// ---------------------------------------------------------------------------------------------
// Configuration and SN-PDUs as a radio sends them
// ---------------------------------------------------------------------------------------------

fn config(packet_data: bool, wap: bool) -> StackConfig {
    let mut cfg = ComponentTest::get_default_test_config(StackMode::Bs);
    cfg.cell.sndcp_service = true;
    cfg.packet_data.enabled = packet_data;
    cfg.wap.enabled = wap;
    cfg
}

fn hex_to_bits(hex: &str) -> String {
    hex.chars().map(|c| format!("{:04b}", c.to_digit(16).unwrap())).collect()
}

/// SN-ACTIVATE PDP CONTEXT DEMAND (table 28.24): dynamic IPv4 unless `static_ip`, with the real
/// Motorola PCO when `pco`.
fn demand(nsapi: u8, static_ip: Option<Ipv4Addr>, pco: bool) -> String {
    let mut s = format!("00000001{nsapi:04b}");
    match static_ip {
        Some(ip) => s.push_str(&format!("000{:032b}", u32::from(ip))),
        None => s.push_str("001"),
    }
    s.push_str("0001"); // packet data MS type B
    s.push_str("00000000"); // PCOMP negotiation
    if pco {
        let pco = hex_to_bits(REAL_PCO_HEX);
        s.push_str("10"); // o-bit, access point name index absent
        s.push_str("10001"); // M-bit, PCO
        s.push_str(&format!("{:011b}", pco.len()));
        s.push_str(&pco);
        s.push('0');
    } else {
        s.push('0');
    }
    s
}

fn transmit_request(nsapi: u8, slots: Option<(u8, bool)>) -> String {
    let resource_request = match slots {
        None => SndcpPacketDataResourceRequest::None,
        Some((n, unspecified)) => SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: n,
            downlink_timeslots: n,
            full_phase_modulation_capability_timeslots: n,
            unspecified_phase_modulation_resource: unspecified,
        }),
    };
    encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi,
        logical_link_status: false,
        resource_request,
    })
    .unwrap()
    .to_bitstr()
}

fn end_of_data() -> String {
    encode_end_of_data(&SndcpEndOfData {
        immediate_service_change: false,
    })
    .unwrap()
    .to_bitstr()
}

/// SN-DEACTIVATE PDP CONTEXT DEMAND (table 28.32) for one NSAPI, or for all.
fn deactivate(nsapi: Option<u8>) -> String {
    match nsapi {
        Some(n) => format!("001000000001{n:04b}0"),
        None => "0010000000000".to_string(),
    }
}

fn unitdata(nsapi: u8, npdu: &[u8]) -> String {
    encode_sn_unitdata(nsapi, 0, 0, &BitBuffer::from_bytes(npdu)).unwrap().to_bitstr()
}

/// SN-DATA (the advanced link's user data PDU).
fn sn_data(nsapi: u8, npdu: &[u8]) -> String {
    encode_sn_data(nsapi, 0, 0, &BitBuffer::from_bytes(npdu)).unwrap().to_bitstr()
}

/// A UDP datagram from the radio at `src` to the gateway's WAP port.
fn datagram(src: Ipv4Addr, dst: Ipv4Addr, port: u16, payload: &[u8]) -> Vec<u8> {
    build_ipv4_udp_npdu(src.octets(), dst.octets(), RADIO_PORT, port, payload, 7, 64).unwrap()
}

/// WTP Invoke, class 2, of a WSP GET (WAP-224 and WAP-230, the MXP600's form).
fn wtp_get(tid: u16, uri: &str) -> Vec<u8> {
    let mut v = vec![0x0a];
    v.extend_from_slice(&(tid & 0x7fff).to_be_bytes());
    v.push(0x12);
    v.push(0x40);
    v.push(uri.len() as u8);
    v.extend_from_slice(uri.as_bytes());
    v
}

/// WTP Invoke, class 2, of a WSP Connect asking for large SDUs, like the MXP600.
fn wtp_connect(tid: u16) -> Vec<u8> {
    let caps = [0x04, 0x80, 0x94, 0x80, 0x00, 0x04, 0x81, 0x94, 0x80, 0x00];
    let mut v = vec![0x0a];
    v.extend_from_slice(&(tid & 0x7fff).to_be_bytes());
    v.extend_from_slice(&[0x12, 0x01, 0x10, caps.len() as u8, 0x00]);
    v.extend_from_slice(&caps);
    v
}

/// WTP Ack from the initiator, with the PSN TPI when `psn` is given.
fn wtp_ack(tid: u16, psn: Option<u8>) -> Vec<u8> {
    let tid = (tid & 0x7fff).to_be_bytes();
    match psn {
        None => vec![0x18, tid[0], tid[1]],
        Some(psn) => vec![0x98, tid[0], tid[1], 0x19, psn],
    }
}

// ---------------------------------------------------------------------------------------------
// The radio on the other side of the MAC
// ---------------------------------------------------------------------------------------------

/// One PDU the LLC handed to the MAC.
#[derive(Debug, Clone)]
struct Down {
    issi: u32,
    llc: LlcPduType,
    link_id: u32,
    /// `TmaUnitdataReq::carrier_num`.
    carrier: Option<u16>,
    stealing: bool,
    chan_alloc: bool,
    alloc: Option<CmceChanAllocReq>,
    /// `TmaUnitdataReq::data_category`.
    category: Option<i32>,
    /// The SN-PDU (bits after the MLE discriminator) when the TL-SDU is for SNDCP. On the advanced
    /// link, the TL-SDU the radio reassembled (a Down of its own after the segments).
    sn: Option<String>,
    /// The LLC PDU (the reassembled TL-SDU for an advanced link one).
    pdu: BitBuffer,
}

impl Down {
    fn sn_type(&self) -> Option<u8> {
        self.sn.as_ref().and_then(|s| u8::from_str_radix(s.get(0..4)?, 2).ok())
    }

    fn sn_buf(&self) -> BitBuffer {
        BitBuffer::from_bitstr(self.sn.as_deref().expect("an SN-PDU"))
    }
}

/// The stack from the LLC up (LLC, MLE, SNDCP) with a simulated radio under the MAC: every PDU the
/// LLC hands down is reported transmitted and every BL-DATA acknowledged, as the air would.
struct Air {
    test: ComponentTest,
    ticks: usize,
    ns: HashMap<u32, u8>,
    acks: Vec<(u32, u8, usize)>,
    /// Report the PDUs transmitted (off: the MAC holds them).
    transmit: bool,
    held: Vec<TxReporter>,
    down: Vec<Down>,
    /// The radio's advanced link: next N(S) it sends, segments it got per (ISSI, N(S)), AL PDUs
    /// it sends up (ISSI, PDU, tick), whether it answers acknowledgement requests, and every
    /// AL-DATA / AL-FINAL that came down (ISSI, header, payload bits).
    al_ns: HashMap<u32, u8>,
    al_rx: HashMap<(u32, u8), (BTreeMap<u8, BitBuffer>, Option<u8>)>,
    al_up: Vec<(u32, BitBuffer, usize)>,
    al_answer: bool,
    /// Radios that do not answer acknowledgement requests (the others do, when `al_answer`).
    al_silent: Vec<u32>,
    al_segments: Vec<(u32, AlData, usize)>,
}

impl Air {
    fn new(cfg: StackConfig) -> Self {
        let mut test = ComponentTest::from_config(cfg, Some(TdmaTime::default()));
        test.populate_entities(
            vec![TetraEntity::Llc, TetraEntity::Mle, TetraEntity::Sndcp],
            vec![TetraEntity::Umac, TetraEntity::Mm, TetraEntity::Cmce],
        );
        Self {
            test,
            ticks: 0,
            ns: HashMap::new(),
            acks: Vec::new(),
            transmit: true,
            held: Vec::new(),
            down: Vec::new(),
            al_ns: HashMap::new(),
            al_rx: HashMap::new(),
            al_up: Vec::new(),
            al_answer: true,
            al_silent: Vec::new(),
            al_segments: Vec::new(),
        }
    }

    fn step(&mut self) {
        let due: Vec<(u32, u8)> = self.acks.iter().filter(|a| a.2 <= self.ticks).map(|a| (a.0, a.1)).collect();
        self.acks.retain(|a| a.2 > self.ticks);
        for (issi, nr) in due {
            let mut pdu = BitBuffer::new_autoexpand(8);
            BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
            pdu.seek(0);
            self.test.submit_message(tma_ind(issi, pdu, 1));
        }
        let due: Vec<(u32, BitBuffer)> = self
            .al_up
            .iter()
            .filter(|a| a.2 <= self.ticks)
            .map(|a| (a.0, a.1.clone()))
            .collect();
        self.al_up.retain(|a| a.2 > self.ticks);
        for (issi, pdu) in due {
            self.test.submit_message(tma_ind(issi, pdu, 1));
        }
        self.test.run_stack(Some(1));
        self.ticks += 1;
        for msg in self.test.dump_sinks() {
            let SapMsgInner::TmaUnitdataReq(req) = msg.msg else { continue };
            let down = parse_down(&req);
            if down.llc == LlcPduType::AlDataAlFinal {
                assert!(
                    !req.stealing_permission && req.chan_alloc.is_none() && req.link_id == 0,
                    "advanced link data on the MCCH, never stolen"
                );
                self.radio_al_segment(down.issi, &req.pdu);
            }
            if let Some(reporter) = req.tx_reporter {
                if self.transmit {
                    if reporter.get_state() == TxState::Pending {
                        reporter.mark_transmitted();
                    }
                } else {
                    self.held.push(reporter);
                }
            }
            if let Some(ns) = down_ns(&req.pdu) {
                self.acks.push((down.issi, ns, self.ticks + 1));
            }
            self.down.push(down);
        }
    }

    /// The radio gets a segment of its advanced link; on an acknowledgement request it answers
    /// with an AL-ACK two slots later and, with the TL-SDU complete, hands it over as a Down.
    fn radio_al_segment(&mut self, issi: u32, pdu: &BitBuffer) {
        let mut pdu = pdu.clone();
        let h = AlData::from_bitbuf(&mut pdu).unwrap();
        let payload = BitBuffer::from_bitbuffer_pos(&pdu);
        self.al_segments.push((issi, h, payload.get_len()));
        let entry = self.al_rx.entry((issi, h.ns)).or_default();
        entry.0.entry(h.ss).or_insert(payload);
        if h.final_segment {
            entry.1 = Some(h.ss);
        }
        if !h.acknowledgement_requested || !self.al_answer || self.al_silent.contains(&issi) {
            return;
        }
        let complete = entry.1.is_some_and(|f| (0..=f).all(|ss| entry.0.contains_key(&ss)));
        let ack = if complete {
            let (segments, _) = self.al_rx.remove(&(issi, h.ns)).unwrap();
            let mut sdu = BitBuffer::new_autoexpand(1024);
            for seg in segments.values() {
                let mut seg = BitBuffer::from_bitbuffer(seg);
                seg.seek(0);
                let len = seg.get_len();
                sdu.copy_bits(&mut seg, len);
            }
            sdu.seek(0);
            assert!(fcs::check_fcs(&sdu), "AL FCS of the TL-SDU");
            let end = sdu.get_raw_end() - 32;
            sdu.set_raw_end(end);
            let bits = sdu.to_bitstr();
            let sn = bits.strip_prefix("100").map(str::to_string);
            self.down.push(Down {
                issi,
                llc: LlcPduType::AlDataAlFinal,
                link_id: 0,
                carrier: None,
                stealing: false,
                chan_alloc: false,
                alloc: None,
                category: None,
                sn,
                pdu: sdu,
            });
            AlAck::complete(h.ns)
        } else {
            let (segments, fin) = &self.al_rx[&(issi, h.ns)];
            let highest = fin.unwrap_or(*segments.keys().next_back().unwrap());
            let sr = (0..=highest).find(|s| !segments.contains_key(s)).unwrap_or(highest + 1);
            let len = (highest.max(sr) - sr + 1).min(62);
            let mut bitmap = 0u64;
            for off in 1..len {
                if segments.contains_key(&(sr + off)) {
                    bitmap |= 1 << (off - 1);
                }
            }
            AlAck::selective(true, h.ns, sr, bitmap, len)
        };
        let mut up = BitBuffer::new_autoexpand(32);
        ack.to_bitbuf(&mut up);
        up.seek(0);
        self.al_up.push((issi, up, self.ticks + 2));
    }

    /// The radio sets up its advanced link 1 (the AL-SETUP of `radio_al_setup`); returns the
    /// station's answer.
    fn al_setup(&mut self, issi: u32) -> AlSetup {
        self.al_setup_with(issi, radio_al_setup())
    }

    fn al_setup_with(&mut self, issi: u32, setup: AlSetup) -> AlSetup {
        let mut pdu = BitBuffer::new_autoexpand(32);
        setup.to_bitbuf(&mut pdu);
        pdu.seek(0);
        self.uplink(issi, pdu);
        for _ in 0..8 {
            if let Some(i) = self.down.iter().position(|d| d.issi == issi && d.llc == LlcPduType::AlSetup) {
                let mut pdu = self.down.remove(i).pdu;
                return AlSetup::from_bitbuf(&mut pdu).unwrap();
            }
            self.step();
        }
        panic!("no AL-SETUP answer");
    }

    /// An SN-PDU on the radio's advanced link: the TL-SDU with its FCS in AL-DATA segments of
    /// 200 bits, the last an AL-FINAL-AR, one per MCCH uplink slot.
    fn send_al(&mut self, issi: u32, sn: &str) {
        let ns = self.al_ns.entry(issi).or_insert(0);
        let this_ns = *ns;
        *ns = (*ns + 1) & 7;
        let mut sdu = BitBuffer::from_bitstr(&format!("100{sn}"));
        let mut with_fcs = BitBuffer::new_autoexpand(sdu.get_len() + 32);
        let len = sdu.get_len();
        with_fcs.copy_bits(&mut sdu, len);
        let value = fcs::compute_fcs(&with_fcs, 0, with_fcs.get_len());
        with_fcs.write_bits(value as u64, 32);
        with_fcs.seek(0);
        let count = with_fcs.get_len().div_ceil(200);
        for ss in 0..count {
            let n = with_fcs.get_len_remaining().min(200);
            let mut pdu = BitBuffer::new_autoexpand(17 + n);
            AlData {
                final_segment: ss + 1 == count,
                acknowledgement_requested: ss + 1 == count,
                ns: this_ns,
                ss: ss as u8,
            }
            .to_bitbuf(&mut pdu);
            pdu.copy_bits(&mut with_fcs, n);
            pdu.seek(0);
            self.uplink(issi, pdu);
        }
    }

    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Send an uplink LLC PDU from `issi` on the MCCH: it goes in on a tick whose uplink slot is
    /// TS1 (downlink TS3, two slots ahead).
    fn uplink(&mut self, issi: u32, llc_pdu: BitBuffer) {
        while self.ticks % 4 != 2 {
            self.step();
        }
        self.test.submit_message(tma_ind(issi, llc_pdu, 1));
        self.step();
    }

    /// Send an uplink LLC PDU from `issi` on timeslot `ts` of the main carrier (downlink slot
    /// two ahead: tick number = ts + 1 modulo 4).
    fn uplink_on(&mut self, issi: u32, llc_pdu: BitBuffer, ts: u8) {
        while self.ticks % 4 != (usize::from(ts) + 1) % 4 {
            self.step();
        }
        self.test.submit_message(tma_ind(issi, llc_pdu, u32::from(ts)));
        self.step();
    }

    /// An SN-PDU on the acknowledged basic link on timeslot `ts`.
    fn send_on(&mut self, issi: u32, sn: &str, ts: u8) {
        let ns = self.ns.entry(issi).or_insert(0);
        let mut pdu = BitBuffer::new_autoexpand(64);
        BlData { has_fcs: false, ns: *ns }.to_bitbuf(&mut pdu);
        *ns ^= 1;
        append_bits(&mut pdu, &format!("100{sn}"));
        self.uplink_on(issi, pdu, ts);
    }

    /// An SN-PDU on the unacknowledged basic link on timeslot `ts`.
    fn send_unack_on(&mut self, issi: u32, sn: &str, ts: u8) {
        let mut pdu = BitBuffer::new_autoexpand(64);
        BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
        append_bits(&mut pdu, &format!("100{sn}"));
        self.uplink_on(issi, pdu, ts);
    }

    /// An SN-PDU on the acknowledged basic link (BL-DATA).
    fn send(&mut self, issi: u32, sn: &str) {
        let ns = self.ns.entry(issi).or_insert(0);
        let mut pdu = BitBuffer::new_autoexpand(64);
        BlData { has_fcs: false, ns: *ns }.to_bitbuf(&mut pdu);
        *ns ^= 1;
        append_bits(&mut pdu, &format!("100{sn}"));
        self.uplink(issi, pdu);
    }

    /// An SN-PDU on the unacknowledged basic link (BL-UDATA).
    fn send_unack(&mut self, issi: u32, sn: &str) {
        let mut pdu = BitBuffer::new_autoexpand(64);
        BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
        append_bits(&mut pdu, &format!("100{sn}"));
        self.uplink(issi, pdu);
    }

    /// The SN-PDUs that came down since the last call.
    fn take_sn(&mut self) -> Vec<Down> {
        std::mem::take(&mut self.down).into_iter().filter(|d| d.sn.is_some()).collect()
    }

    /// Run until an SN-PDU comes down (at most `ticks`).
    fn next_sn(&mut self, ticks: usize) -> Option<Down> {
        for _ in 0..ticks {
            if let Some(i) = self.down.iter().position(|d| d.sn.is_some()) {
                return Some(self.down.remove(i));
            }
            self.step();
        }
        let i = self.down.iter().position(|d| d.sn.is_some())?;
        Some(self.down.remove(i))
    }

    /// Activate a PDP context for `issi` and bring it to READY; returns its address.
    fn attach(&mut self, issi: u32, nsapi: u8) -> Ipv4Addr {
        self.send(issi, &demand(nsapi, None, false));
        let accept = self.next_sn(8).expect("ACCEPT");
        let ip = accept_ip(&accept);
        self.send(issi, &transmit_request(nsapi, Some((1, false))));
        let response = self.next_sn(8).expect("RESPONSE");
        assert_eq!(
            decode_data_transmit_response(&response.sn_buf()).unwrap().result,
            SndcpDataTransmitResponseResult::Accepted
        );
        ip
    }
}

fn append_bits(pdu: &mut BitBuffer, bits: &str) {
    let mut src = BitBuffer::from_bitstr(bits);
    pdu.copy_bits(&mut src, bits.len());
    pdu.seek(0);
}

fn tma_ind(issi: u32, pdu: BitBuffer, ts: u32) -> SapMsg {
    SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Umac,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TmaUnitdataInd(TmaUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu: Some(pdu),
            main_address: TetraAddress::new(issi, SsiType::Issi),
            scrambling_code: 0,
            link_id: ts,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            air_interface_encryption: 0,
            chan_change_response_req: false,
            chan_change_handle: None,
            chan_info: None,
        }),
    }
}

/// N(S) of a BL-DATA / BL-ADATA going down.
fn down_ns(pdu: &BitBuffer) -> Option<u8> {
    let mut pdu = pdu.clone();
    match LlcPduType::try_from(pdu.peek_bits(4)?).ok()? {
        LlcPduType::BlData => BlData::from_bitbuf(&mut pdu).ok().map(|p| p.ns),
        LlcPduType::BlAdata => BlAdata::from_bitbuf(&mut pdu).ok().map(|p| p.ns),
        _ => None,
    }
}

fn parse_down(req: &TmaUnitdataReq) -> Down {
    let mut pdu = req.pdu.clone();
    let llc = LlcPduType::try_from(pdu.peek_bits(4).unwrap()).unwrap();
    if matches!(
        llc,
        LlcPduType::AlSetup | LlcPduType::AlDataAlFinal | LlcPduType::AlAckAlRnr | LlcPduType::AlReconnect | LlcPduType::AlDisc
    ) {
        return Down {
            issi: req.main_address.ssi,
            llc,
            link_id: req.link_id,
            carrier: req.carrier_num,
            stealing: req.stealing_permission,
            chan_alloc: req.chan_alloc.is_some(),
            alloc: req.chan_alloc.clone(),
            category: req.data_category,
            sn: None,
            pdu,
        };
    }
    match llc {
        LlcPduType::BlData => {
            BlData::from_bitbuf(&mut pdu).unwrap();
        }
        LlcPduType::BlAdata => {
            BlAdata::from_bitbuf(&mut pdu).unwrap();
        }
        LlcPduType::BlUdata => {
            BlUdata::from_bitbuf(&mut pdu).unwrap();
        }
        LlcPduType::BlAck => {
            BlAck::from_bitbuf(&mut pdu).unwrap();
        }
        other => panic!("unexpected LLC PDU {other:?} going down"),
    }
    let sn = (pdu.get_len_remaining() >= 3 && pdu.read_bits(3) == Some(MleProtocolDiscriminator::Sndcp.into_raw()))
        .then(|| BitBuffer::from_bitbuffer_pos(&pdu).to_bitstr());
    Down {
        issi: req.main_address.ssi,
        llc,
        link_id: req.link_id,
        carrier: req.carrier_num,
        stealing: req.stealing_permission,
        chan_alloc: req.chan_alloc.is_some(),
        alloc: req.chan_alloc.clone(),
        category: req.data_category,
        sn,
        pdu: req.pdu.clone(),
    }
}

/// The AL-SETUP a radio sends for its original acknowledged advanced link 1: N.271 2048 octets,
/// no slot request, window 1, N.273 3, N.274 3.
fn radio_al_setup() -> AlSetup {
    AlSetup {
        acknowledged_service: true,
        advanced_link_number: 0,
        max_tl_sdu_len_code: 6,
        connection_width: false,
        advanced_link_symmetry: false,
        uplink_timeslots: None,
        downlink_timeslots: None,
        throughput_code: 6,
        window_size_code: 1,
        max_tl_sdu_retransmissions: 3,
        max_segment_retransmissions: 3,
        setup_report: AlSetup::SETUP_REPORT_SERVICE_DEFINITION,
        ns: None,
        augmented: None,
    }
}

/// Fields of an SN-ACTIVATE PDP CONTEXT ACCEPT (table 28.23): (NSAPI, READY, TIA, IPv4, MTU code,
/// rest after the MTU).
fn accept_fields(sn: &str) -> (u8, u8, u8, Ipv4Addr, u8, String) {
    let f = |o: usize, n: usize| u64::from_str_radix(&sn[o..o + n], 2).unwrap();
    assert_eq!(f(0, 4), 0, "SN-ACTIVATE PDP CONTEXT ACCEPT: {sn}");
    (
        f(4, 4) as u8,
        f(11, 4) as u8,
        f(23, 3) as u8,
        Ipv4Addr::from(f(26, 32) as u32),
        f(66, 3) as u8,
        sn[69..].to_string(),
    )
}

fn accept_ip(down: &Down) -> Ipv4Addr {
    accept_fields(down.sn.as_deref().unwrap()).3
}

/// The WTP packet inside a downlink SN-UNITDATA, with the IPv4 datagram's size.
fn wtp_down(down: &Down, radio_ip: Ipv4Addr) -> (Vec<u8>, usize) {
    let expected = if down.llc == LlcPduType::AlDataAlFinal { 5 } else { 4 };
    assert_eq!(
        down.sn_type(),
        Some(expected),
        "SN-UNITDATA on the basic link, SN-DATA on the advanced link"
    );
    let unitdata = decode_sn_user_data_pdu(&down.sn_buf()).unwrap();
    let mut npdu = unitdata.n_pdu.clone();
    let mut octets = Vec::new();
    while let Some(b) = npdu.read_bits(8) {
        octets.push(b as u8);
    }
    let ip = parse_ipv4_packet(&octets).unwrap();
    assert_eq!((Ipv4Addr::from(ip.source), Ipv4Addr::from(ip.destination)), (GATEWAY, radio_ip));
    let udp = parse_udp_datagram(ip.payload).unwrap();
    assert_eq!(udp.destination_port, RADIO_PORT);
    (udp.payload.to_vec(), octets.len())
}

// ---------------------------------------------------------------------------------------------
// MLE routing (LTPD bearer)
// ---------------------------------------------------------------------------------------------

fn tl_ind(pd: MleProtocolDiscriminator, unack: bool) -> SapMsg {
    let mut sdu = BitBuffer::new(11);
    sdu.write_bits(pd.into_raw(), 3);
    sdu.write_bits(0b1010_1100, 8);
    sdu.seek(0);
    let addr = TetraAddress::new(ISSI, SsiType::Issi);
    let msg = if unack {
        SapMsgInner::TlaTlUnitdataIndBl(TlaTlUnitdataIndBl {
            main_address: addr,
            link_id: 0,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            report: None,
        })
    } else {
        SapMsgInner::TlaTlDataIndBl(TlaTlDataIndBl {
            main_address: addr,
            link_id: 1,
            endpoint_id: 0,
            new_endpoint_id: None,
            css_endpoint_id: None,
            tl_sdu: Some(sdu),
            scrambling_code: 0,
            fcs_flag: false,
            air_interface_encryption: 0,
            chan_change_resp_req: false,
            chan_change_handle: None,
            chan_info: None,
            req_handle: 0,
        })
    };
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg,
    }
}

fn through_mle(packet_data: bool, msg: SapMsg) -> Vec<SapMsg> {
    let mut test = ComponentTest::from_config(config(packet_data, false), None);
    test.populate_entities(
        vec![TetraEntity::Mle],
        vec![TetraEntity::Sndcp, TetraEntity::Mm, TetraEntity::Cmce, TetraEntity::Llc],
    );
    test.submit_message(msg);
    test.deliver_all_messages();
    test.dump_sinks()
}

#[test]
fn tl_unitdata_for_sndcp_is_dropped_with_packet_data_off() {
    assert!(through_mle(false, tl_ind(MleProtocolDiscriminator::Sndcp, true)).is_empty());
}

#[test]
fn tl_unitdata_for_sndcp_reaches_sndcp_as_basic_unack() {
    let out = through_mle(true, tl_ind(MleProtocolDiscriminator::Sndcp, true));
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].sap, out[0].dest), (Sap::TlpdSap, TetraEntity::Sndcp));
    let SapMsgInner::LtpdMleUnitdataInd(ind) = &out[0].msg else {
        panic!("LTPD indication expected");
    };
    assert_eq!(ind.bearer, LtpdBearer::BasicUnack);
    assert_eq!(ind.received_tetra_address.ssi, ISSI);
    assert_eq!(ind.sdu.peek_bits(8), Some(0b1010_1100), "cursor after the MLE discriminator");
}

#[test]
fn tl_unitdata_for_cmce_or_mm_is_still_dropped() {
    for pd in [MleProtocolDiscriminator::Cmce, MleProtocolDiscriminator::Mm] {
        assert!(through_mle(true, tl_ind(pd, true)).is_empty(), "{pd:?}");
    }
}

#[test]
fn tl_data_for_sndcp_is_basic_ack() {
    for packet_data in [false, true] {
        let out = through_mle(packet_data, tl_ind(MleProtocolDiscriminator::Sndcp, false));
        assert_eq!(out.len(), 1);
        let SapMsgInner::LtpdMleUnitdataInd(ind) = &out[0].msg else {
            panic!("LTPD indication expected");
        };
        assert_eq!((ind.bearer, ind.link_id), (LtpdBearer::BasicAck, 1));
    }
}

fn tl_data_al_ind(pd: MleProtocolDiscriminator) -> SapMsg {
    let mut sdu = BitBuffer::new(11);
    sdu.write_bits(pd.into_raw(), 3);
    sdu.write_bits(0b1010_1100, 8);
    sdu.seek(0);
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Mle,
        msg: SapMsgInner::TlaTlDataIndAl(TlDataIndAl {
            main_address: TetraAddress::new(ISSI, SsiType::Issi),
            al_number: 0,
            max_sdu_bytes: 2048,
            link_id: 1,
            carrier_num: MAIN_CARRIER,
            tl_sdu: sdu,
        }),
    }
}

#[test]
fn tl_data_on_an_advanced_link_reaches_sndcp_as_advanced() {
    let out = through_mle(true, tl_data_al_ind(MleProtocolDiscriminator::Sndcp));
    assert_eq!(out.len(), 1);
    assert_eq!((out[0].sap, out[0].dest), (Sap::TlpdSap, TetraEntity::Sndcp));
    let SapMsgInner::LtpdMleUnitdataInd(ind) = &out[0].msg else {
        panic!("LTPD indication expected");
    };
    assert_eq!(
        (ind.bearer, ind.link_id),
        (
            LtpdBearer::Advanced {
                al_number: 0,
                max_sdu_bytes: 2048
            },
            1
        )
    );
    assert_eq!(ind.sdu.peek_bits(8), Some(0b1010_1100), "cursor after the MLE discriminator");
}

#[test]
fn tl_data_on_an_advanced_link_for_cmce_or_mm_is_dropped() {
    for pd in [MleProtocolDiscriminator::Cmce, MleProtocolDiscriminator::Mm] {
        assert!(through_mle(true, tl_data_al_ind(pd)).is_empty(), "{pd:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Advanced link in the LLC
// ---------------------------------------------------------------------------------------------

fn al_pdu(write: impl FnOnce(&mut BitBuffer)) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(64);
    write(&mut pdu);
    pdu.seek(0);
    pdu
}

/// With `[packet_data]` off, the advanced link PDUs of a radio make nothing go down, exactly as
/// before the advanced link existed.
#[test]
fn advanced_link_pdus_produce_nothing_with_packet_data_off() {
    use tetra_pdus::llc::pdus::al_disc::{AlDisc, AlDiscReport};
    use tetra_pdus::llc::pdus::al_reconnect::{AlReconnect, AlReconnectReport};
    let mut air = Air::new(config(false, false));
    air.uplink(ISSI, al_pdu(|b| radio_al_setup().to_bitbuf(b)));
    air.uplink(
        ISSI,
        al_pdu(|b| {
            AlData {
                final_segment: true,
                acknowledgement_requested: true,
                ns: 0,
                ss: 0,
            }
            .to_bitbuf(b);
            b.write_bits(0xdead_beef_0123, 48);
        }),
    );
    air.uplink(ISSI, al_pdu(|b| AlAck::complete(0).to_bitbuf(b)));
    air.uplink(
        ISSI,
        al_pdu(|b| {
            AlReconnect {
                acknowledged_service: true,
                advanced_link_number: 0,
                report: AlReconnectReport::Propose,
            }
            .to_bitbuf(b)
        }),
    );
    air.uplink(
        ISSI,
        al_pdu(|b| {
            AlDisc {
                acknowledged_service: true,
                advanced_link_number: 0,
                report: AlDiscReport::Close,
            }
            .to_bitbuf(b)
        }),
    );
    air.run(8);
    assert!(air.down.is_empty(), "{:?}", air.down);
}

/// With `[packet_data]` on, AL-SETUP is answered on the MCCH with the radio's own parameters.
#[test]
fn advanced_link_setup_is_answered_with_packet_data_on() {
    debug::setup_logging_verbose();
    let mut air = Air::new(config(true, false));
    let answer = air.al_setup(ISSI);
    assert_eq!(answer.setup_report, AlSetup::SETUP_REPORT_SUCCESS);
    assert_eq!(
        AlSetup {
            setup_report: AlSetup::SETUP_REPORT_SERVICE_DEFINITION,
            ..answer
        },
        radio_al_setup()
    );
    assert!(air.down.iter().all(|d| d.link_id == 0 && !d.stealing && !d.chan_alloc));
}

/// An SN-PDU the radio sends on its advanced link reaches SNDCP (here an SN-DATA TRANSMIT
/// REQUEST, which is answered on the basic link) and the TL-SDU is acknowledged on the MCCH.
#[test]
fn an_sn_pdu_on_the_advanced_link_reaches_sndcp() {
    debug::setup_logging_verbose();
    let mut air = Air::new(config(true, true));
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.al_setup(ISSI);
    air.take_sn();
    air.send_al(ISSI, &transmit_request(1, None));
    let response = air.next_sn(8).expect("RESPONSE");
    assert!(
        matches!(response.llc, LlcPduType::BlData | LlcPduType::BlAdata),
        "transfer control on the acknowledged basic link: {:?}",
        response.llc
    );
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    let ack = air
        .down
        .iter()
        .find(|d| d.llc == LlcPduType::AlAckAlRnr)
        .map(|d| AlAck::from_bitbuf(&mut d.pdu.clone()).unwrap())
        .expect("AL-ACK for the AL-FINAL-AR");
    assert!(ack.acknowledges_complete_tl_sdu() && ack.nr == 0);
}

// The advanced link through the real MAC -------------------------------------------------------

/// MAC-RESOURCE PDUs of a downlink block: (SSI, length indication, slot grant, LLC PDU type).
fn resources_in(block: &BitBuffer) -> Vec<(u32, u8, bool, Option<LlcPduType>)> {
    use tetra_pdus::umac::pdus::mac_resource::MacResource;
    let mut out = Vec::new();
    let len = block.get_len();
    let mut pos = 0;
    while pos + 16 <= len {
        let mut b = BitBuffer::from_bitbuffer(block);
        b.seek(pos);
        if b.peek_bits(2) != Some(0) {
            break;
        }
        let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
        let Some(addr) = res.addr else { break };
        let llc = b.peek_bits(4).and_then(|t| LlcPduType::try_from(t).ok());
        out.push((addr.ssi, res.length_ind, res.slot_granting_element.is_some(), llc));
        if res.length_ind == 0b111111 {
            break;
        }
        pos += res.length_ind as usize * 8;
    }
    out
}

/// UMAC and LLC (with MLE as a sink) and the LMAC collecting what goes on the air.
struct MacAir {
    test: ComponentTest,
    tick: usize,
}

/// One downlink block: (tick, carrier, timeslot, MAC-RESOURCEs).
type AirBlock = (usize, u16, u8, Vec<(u32, u8, bool, Option<LlcPduType>)>);

impl MacAir {
    fn new() -> Self {
        Self::with(config(true, false))
    }

    fn with(cfg: StackConfig) -> Self {
        let mut test = ComponentTest::from_config(cfg, Some(TdmaTime { h: 0, m: 1, f: 1, t: 1 }));
        test.populate_entities(vec![TetraEntity::Umac, TetraEntity::Llc], vec![TetraEntity::Lmac, TetraEntity::Mle]);
        Self { test, tick: 0 }
    }

    /// Main-carrier downlink slots on `ts` (frames 1 to 17) in the next `ticks` ticks.
    fn run_slots(&mut self, ticks: usize, ts: u8) -> Vec<tetra_saps::tmv::TmvUnitdataReqSlot> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            self.test.run_stack(Some(1));
            self.tick += 1;
            for msg in self.test.dump_sinks() {
                let slots = match msg.msg {
                    SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
                    SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
                    _ => continue,
                };
                out.extend(
                    slots
                        .into_iter()
                        .filter(|s| s.carrier_num == MAIN_CARRIER && s.ts.t == ts && s.ts.f != 18),
                );
            }
        }
        out
    }

    fn run(&mut self, ticks: usize) -> Vec<AirBlock> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            self.test.run_stack(Some(1));
            self.tick += 1;
            for msg in self.test.dump_sinks() {
                let slots = match msg.msg {
                    SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
                    SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
                    _ => continue,
                };
                for slot in slots {
                    for blk in [&slot.blk1, &slot.blk2].into_iter().flatten() {
                        out.push((self.tick, slot.carrier_num, slot.ts.t, resources_in(&blk.mac_block)));
                    }
                }
            }
        }
        out
    }

    fn uplink(&mut self, pdu: BitBuffer) {
        let mut msg = tma_ind(ISSI, pdu, 1);
        let SapMsgInner::TmaUnitdataInd(ind) = &mut msg.msg else {
            unreachable!()
        };
        ind.carrier_num = self.test.config.config().cell.main_carrier;
        self.test.submit_message(msg);
    }

    /// AL-SETUP from the radio, then a TL-SDU of `octets` for it on the advanced link.
    fn transfer(&mut self, octets: usize) -> TxReporter {
        self.uplink(al_pdu(|b| radio_al_setup().to_bitbuf(b)));
        let setup = self.run(12);
        assert!(
            setup
                .iter()
                .any(|b| b.3.iter().any(|r| r.0 == ISSI && r.3 == Some(LlcPduType::AlSetup))),
            "AL-SETUP answered on the air"
        );
        let reporter = TxReporter::new();
        self.test.submit_message(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Llc,
            msg: SapMsgInner::TlaTlDataReqAl(tetra_saps::tla::TlDataReqAl {
                main_address: TetraAddress::new(ISSI, SsiType::Issi),
                al_number: 0,
                tl_sdu: BitBuffer::from_bytes(&vec![0x3c; octets]),
                tx_reporter: Some(reporter.clone()),
            }),
        });
        reporter
    }
}

fn al_segments_on_air(blocks: &[AirBlock]) -> Vec<(u16, u8, u8)> {
    blocks
        .iter()
        .flat_map(|(_, carrier, ts, res)| {
            res.iter()
                .filter(|r| r.0 == ISSI && r.3 == Some(LlcPduType::AlDataAlFinal))
                .map(move |r| (*carrier, *ts, r.1))
        })
        .collect()
}

/// Every AL segment goes out whole in one MCCH block (no MAC fragmentation), one per frame,
/// and the whole TL-SDU reaches the air.
#[test]
fn advanced_link_segments_fill_one_mcch_block_each() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::new();
    let reporter = mac.transfer(300);
    let blocks = mac.run(4 * 20);
    let segments = al_segments_on_air(&blocks);
    // 300 octets + FCS = 2432 bits in segments of 194.
    assert_eq!(segments.len(), 13, "{segments:?}");
    for (carrier, ts, length_ind) in &segments {
        assert_eq!((*carrier, *ts), (MAIN_CARRIER, 1), "on the MCCH");
        assert_ne!(*length_ind, 0b111111, "never fragmented by the MAC");
    }
    assert_eq!(reporter.get_state(), TxState::Transmitted);
}

/// A D-SETUP for a group queued in the middle of a long AL transfer goes out within the next two
/// MCCH blocks: the AL keeps at most one segment waiting in the MAC.
#[test]
fn group_call_setup_is_not_held_behind_an_al_transfer() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::new();
    mac.transfer(400);
    let before = mac.run(4 * 5);
    assert!(al_segments_on_air(&before).len() >= 3, "the transfer is under way");
    let mut pdu = BitBuffer::new_autoexpand(128);
    BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    pdu.write_bits(0b010, 3); // CMCE
    pdu.write_bits(0x1234_5678_9abc_def0, 64);
    pdu.write_bits(0x0fed_cba9_8765, 48);
    pdu.seek(0);
    mac.test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            carrier_num: Some(MAIN_CARRIER),
            req_handle: 0,
            pdu,
            main_address: TetraAddress::new(GSSI, SsiType::Gssi),
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });
    let after = mac.run(4 * 6);
    let mcch: Vec<&AirBlock> = after
        .iter()
        .filter(|b| b.1 == MAIN_CARRIER && b.2 == 1 && !b.3.is_empty())
        .collect();
    let at = mcch
        .iter()
        .position(|b| b.3.iter().any(|r| r.0 == GSSI))
        .expect("the group PDU went out");
    assert!(at < 2, "the group PDU waited {} MCCH blocks: {:?}", at, &mcch[..=at]);
    assert!(al_segments_on_air(&after).len() >= 3, "the transfer goes on after it");
}

/// A group PDU queued while a basic-link datagram of 300 octets (about ten MCCH blocks) is being
/// fragmented goes out within the next two MCCH blocks, ahead of the datagram's next fragment,
/// and the datagram still completes.
#[test]
fn group_call_setup_is_not_held_behind_a_fragmented_datagram() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::new();
    let reporter = TxReporter::new_unacked();
    mac.test
        .submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 300, Some(reporter.clone())));
    let before = mac.run(4 * 3);
    assert!(
        before.iter().any(|b| b.3.iter().any(|r| r.0 == ISSI && r.1 == 0b111111)),
        "the datagram's fragmentation started"
    );
    let mut pdu = BitBuffer::new_autoexpand(128);
    BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    pdu.write_bits(0b010, 3); // CMCE
    pdu.write_bits(0x1234_5678_9abc_def0, 64);
    pdu.write_bits(0x0fed_cba9_8765, 48);
    pdu.seek(0);
    mac.test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            carrier_num: Some(MAIN_CARRIER),
            req_handle: 0,
            pdu,
            main_address: TetraAddress::new(GSSI, SsiType::Gssi),
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });
    let after = mac.run(4 * 16);
    let mcch: Vec<&AirBlock> = after.iter().filter(|b| b.1 == MAIN_CARRIER && b.2 == 1).collect();
    let at = mcch
        .iter()
        .position(|b| b.3.iter().any(|r| r.0 == GSSI))
        .expect("the group PDU went out");
    assert!(at < 2, "the group PDU waited {} MCCH blocks: {:?}", at, &mcch[..=at]);
    assert_eq!(reporter.get_state(), TxState::Transmitted, "the datagram completed");
}

/// With a call on timeslot 2, nothing of the advanced link is stolen from it: every AL PDU goes
/// out on the MCCH.
#[test]
fn advanced_link_never_steals_from_a_call() {
    use tetra_core::Direction;
    use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
    let mut mac = MacAir::new();
    mac.test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            carrier_num: MAIN_CARRIER,
            ts: 2,
            peer_carrier_num: None,
            peer_ts: None,
            usage: 4,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::SwMI,
        })),
    });
    mac.run(4);
    mac.transfer(120);
    let blocks = mac.run(4 * 12);
    assert_eq!(al_segments_on_air(&blocks).len(), 6);
    for (_, carrier, ts, res) in &blocks {
        if res.iter().any(|r| r.0 == ISSI) {
            assert_eq!((*carrier, *ts), (MAIN_CARRIER, 1), "AL PDU outside the MCCH: {res:?}");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Runtime: PDP contexts, READY / STANDBY, transfer control
// ---------------------------------------------------------------------------------------------

#[test]
fn demand_with_real_pco_gets_pool_address_and_chap_success() {
    debug::setup_logging_verbose();
    let mut air = Air::new(config(true, true));
    air.send(ISSI, &demand(1, None, true));
    let accept = air.next_sn(8).expect("ACCEPT");
    assert_eq!(
        (accept.issi, accept.llc == LlcPduType::BlUdata),
        (ISSI, false),
        "acknowledged basic link"
    );
    assert!(!accept.chan_alloc && !accept.stealing);
    let (nsapi, ready, tia, ip, mtu, rest) = accept_fields(accept.sn.as_deref().unwrap());
    assert_eq!((nsapi, ready, tia, ip, mtu), (1, 10, 2, Ipv4Addr::new(10, 0, 0, 2), 2));
    // The optional section is the stub's CHAP Success, identifier 5 echoed.
    assert_eq!(rest.len(), 81);
    assert_eq!(&rest[..5], "10001");
    assert_eq!(&rest[48..64], "0000001100000101", "CHAP Success, id 5");
}

#[test]
fn two_radios_get_distinct_addresses_and_static_is_granted() {
    let mut air = Air::new(config(true, false));
    air.send(ISSI, &demand(1, None, false));
    let a = accept_ip(&air.next_sn(8).unwrap());
    air.send(ISSI2, &demand(1, None, false));
    let b = accept_ip(&air.next_sn(8).unwrap());
    assert_eq!((a, b), (Ipv4Addr::new(10, 0, 0, 2), Ipv4Addr::new(10, 0, 0, 3)));
    air.send(3_000_001, &demand(2, Some(Ipv4Addr::new(10, 9, 9, 9)), false));
    let accept = air.next_sn(8).unwrap();
    let (_, _, tia, ip, _, rest) = accept_fields(accept.sn.as_deref().unwrap());
    assert_eq!((tia, ip, rest.as_str()), (1, Ipv4Addr::new(10, 9, 9, 9), "0"));
    // A static request for an address another radio holds is refused (cause 9).
    air.send(3_000_002, &demand(1, Some(a), false));
    let reject = air.next_sn(8).unwrap();
    assert_eq!(
        reject.sn.as_deref(),
        Some("00110001000010010"),
        "REJECT, NSAPI 1, cause 9 (static address in use)"
    );
}

#[test]
fn full_pool_rejects_and_redemand_replaces_the_context() {
    let mut cfg = config(true, false);
    cfg.packet_data.pool_first = Ipv4Addr::new(10, 0, 0, 7);
    cfg.packet_data.pool_last = Ipv4Addr::new(10, 0, 0, 7);
    let mut air = Air::new(cfg);
    air.send(ISSI, &demand(1, None, false));
    assert_eq!(accept_ip(&air.next_sn(8).unwrap()), Ipv4Addr::new(10, 0, 0, 7));
    air.send(ISSI2, &demand(1, None, false));
    let reject = air.next_sn(8).unwrap();
    assert_eq!(reject.sn.as_deref(), Some("00110001000001110"), "REJECT, cause 7 (pool empty)");
    // The same radio activating again gets the address back (the old context is released).
    air.send(ISSI, &demand(1, None, false));
    assert_eq!(accept_ip(&air.next_sn(8).unwrap()), Ipv4Addr::new(10, 0, 0, 7));
}

#[test]
fn deactivate_returns_the_address_to_the_pool() {
    let mut cfg = config(true, false);
    cfg.packet_data.pool_last = Ipv4Addr::new(10, 0, 0, 2);
    let mut air = Air::new(cfg);
    air.send(ISSI, &demand(3, None, false));
    air.next_sn(8).unwrap();
    air.send(ISSI, &deactivate(Some(3)));
    let accept = air.next_sn(8).unwrap();
    assert_eq!(accept.sn.as_deref(), Some("00010000000100110"), "DEACTIVATE ACCEPT, NSAPI 3");
    air.send(ISSI2, &demand(1, None, false));
    assert_eq!(accept_ip(&air.next_sn(8).unwrap()), Ipv4Addr::new(10, 0, 0, 2));
    // "Deactivate all" for a radio with nothing active is still answered.
    air.send(ISSI, &deactivate(None));
    assert_eq!(air.next_sn(8).unwrap().sn.as_deref(), Some("0001000000000"));
}

#[test]
fn transmit_request_and_reconnect_accepted_without_channel() {
    let mut air = Air::new(config(true, false));
    // Without a context: rejected, unknown NSAPI.
    air.send(ISSI, &transmit_request(1, None));
    let rejected = air.next_sn(8).unwrap();
    assert_eq!(
        decode_data_transmit_response(&rejected.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::UnknownNsapi)
    );
    air.send(ISSI, &demand(1, None, true));
    air.next_sn(8).unwrap();
    // The MXP600's forms: one slot, four unspecified, four specific.
    for slots in [Some((1, false)), Some((4, true)), Some((4, false)), None] {
        air.send(ISSI, &transmit_request(1, slots));
        let response = air.next_sn(8).expect("RESPONSE");
        assert_eq!(response.llc == LlcPduType::BlUdata, false, "acknowledged basic link");
        assert!(!response.chan_alloc, "no channel assignment on the MCCH");
        assert_eq!(response.link_id, 0);
        let decoded = decode_data_transmit_response(&response.sn_buf()).unwrap();
        assert_eq!((decoded.nsapi, decoded.result), (1, SndcpDataTransmitResponseResult::Accepted));
    }
    let reconnect = encode_reconnect(&SndcpReconnect {
        nsapi: Some(1),
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: 4,
            downlink_timeslots: 4,
            full_phase_modulation_capability_timeslots: 4,
            unspecified_phase_modulation_resource: false,
        }),
    })
    .unwrap()
    .to_bitstr();
    air.send(ISSI, &reconnect);
    let response = air.next_sn(8).unwrap();
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    // END OF DATA from the radio is answered with END OF DATA.
    air.send(ISSI, &end_of_data());
    let eod = air.next_sn(8).unwrap();
    assert!(!decode_end_of_data(&eod.sn_buf()).unwrap().immediate_service_change);
}

/// READY expiry on the TDMA clock (the STANDBY expiry is a unit test of the runtime: 10 min of
/// ticks is too long here).
#[test]
fn ready_expiry_sends_end_of_data() {
    let mut cfg = config(true, false);
    cfg.packet_data.ready_timer_code = 8; // 10 s announced, 8 s here
    let mut air = Air::new(cfg);
    air.attach(ISSI, 1);
    air.take_sn();
    // 8 s = 565 slots, plus up to one housekeeping period; not before.
    air.run(560);
    assert!(air.take_sn().is_empty(), "READY still running");
    let eod = air.next_sn(80).expect("END OF DATA at READY expiry");
    assert_eq!((eod.issi, eod.sn_type(), eod.llc == LlcPduType::BlUdata), (ISSI, Some(8), false));
    assert!(!decode_end_of_data(&eod.sn_buf()).unwrap().immediate_service_change);
    // In STANDBY the context is kept: a new TRANSMIT REQUEST brings it back to READY.
    air.send(ISSI, &transmit_request(1, None));
    let ok = air.next_sn(8).unwrap();
    assert_eq!(
        decode_data_transmit_response(&ok.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
}

/// Activating the same NSAPI again or deactivating it drops the radio's WSP sessions in the
/// gateway (seen in the dashboard status the gateway publishes once a second).
#[test]
fn redemand_and_deactivate_drop_the_gateway_sessions() {
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    let sessions = |air: &mut Air| -> usize {
        std::thread::sleep(std::time::Duration::from_millis(1100));
        air.run(1);
        air.test
            .config
            .state_read()
            .wap_status
            .sessions
            .iter()
            .filter(|s| s.issi == ISSI)
            .count()
    };
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_connect(0x60))));
    let reply = air.next_sn(200).expect("ConnectReply");
    let (wtp, _) = wtp_down(&reply, ip);
    assert_eq!(&wtp[..4], &[0x12, 0x80, 0x60, 0x02], "WTP Result with WSP ConnectReply");
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(0x60, None))));
    assert_eq!(sessions(&mut air), 1);
    air.send(ISSI, &demand(1, None, false));
    assert_eq!(accept_ip(&air.next_sn(8).unwrap()), ip, "same address again");
    assert_eq!(sessions(&mut air), 0, "re-activation drops the session");

    air.send(ISSI, &transmit_request(1, None));
    air.next_sn(8).unwrap();
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_connect(0x61))));
    air.next_sn(200).expect("ConnectReply");
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(0x61, None))));
    assert_eq!(sessions(&mut air), 1);
    air.send(ISSI, &deactivate(Some(1)));
    air.next_sn(8).unwrap();
    assert_eq!(sessions(&mut air), 0, "deactivation drops the session");
}

#[test]
fn deregistration_releases_the_context() {
    let mut air = Air::new(config(true, false));
    air.test.config.state_write().subscribers.register(ISSI);
    air.attach(ISSI, 1);
    air.run(80); // seen registered
    air.test.config.state_write().subscribers.deregister(ISSI);
    air.run(80);
    air.take_sn();
    air.send(ISSI, &transmit_request(1, None));
    assert!(matches!(
        decode_data_transmit_response(&air.next_sn(8).unwrap().sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Rejected(_)
    ));
}

#[test]
fn unsupported_types_get_not_supported_and_garbage_gets_nothing() {
    let mut air = Air::new(config(true, true));
    air.send(ISSI, "11010000000000"); // SN-MODIFY
    let ns = air.next_sn(8).unwrap();
    assert_eq!(decode_not_supported(&ns.sn_buf()).unwrap().not_supported_pdu_type, 13);
    for garbage in [
        "",
        "0",
        "0000",
        "0010",
        "0100",
        "0100000100000000",
        "0110",
        "1001",
        "1111",
        "0101000100000000",
        "0111",
        "1011",
    ] {
        air.send(ISSI, garbage);
        air.run(8);
        assert!(air.take_sn().is_empty(), "{garbage:?} must get no answer");
    }
}

// ---------------------------------------------------------------------------------------------
// End to end: a WSP GET from a radio to the gateway's home page and back
// ---------------------------------------------------------------------------------------------

#[test]
fn wsp_get_of_the_home_page_end_to_end() {
    debug::setup_logging_verbose();
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    assert_eq!(ip, Ipv4Addr::new(10, 0, 0, 2));
    air.take_sn();

    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x21, "/"))));
    let mut parts: std::collections::BTreeMap<u8, Vec<u8>> = std::collections::BTreeMap::new();
    let mut packets = 0;
    loop {
        let down = air.next_sn(400).expect("the gateway's answer comes down");
        assert_eq!(
            (down.issi, down.llc, down.link_id),
            (ISSI, LlcPduType::BlUdata, 0),
            "SN-UNITDATA on the MCCH"
        );
        assert!(!down.stealing && !down.chan_alloc);
        let (wtp, size) = wtp_down(&down, ip);
        assert!(size <= 320, "datagram of {size} bytes over the basic link");
        packets += 1;
        let ty = (wtp[0] >> 3) & 0x0f;
        let flags = wtp[0] & 0x06;
        assert_eq!(u16::from_be_bytes([wtp[1], wtp[2]]), 0x8021);
        let (psn, data) = match ty {
            2 => (0, &wtp[3..]),
            6 => (wtp[3], &wtp[4..]),
            _ => panic!("WTP PDU type {ty}: {wtp:02x?}"),
        };
        parts.insert(psn, data.to_vec());
        if flags & 0x02 != 0 {
            air.send_unack(
                ISSI,
                &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(0x21, (ty == 6).then_some(psn)))),
            );
            break;
        }
        if flags & 0x04 != 0 {
            air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(0x21, Some(psn)))));
        }
    }
    assert!(packets > 1, "the home page does not fit one basic-link datagram");
    let reply: Vec<u8> = parts.into_values().flatten().collect();
    assert_eq!(&reply[..2], &[0x04, 0x20], "WSP Reply, 200 OK");
    let body = String::from_utf8_lossy(&reply);
    assert!(body.contains("FlowStation") && body.trim_end().ends_with("</html>"), "{body}");
    // The final Ack ends the transaction: nothing more comes down.
    air.run(200);
    assert!(air.take_sn().is_empty());
}

#[test]
fn spoofed_source_or_other_destination_gets_nothing() {
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    air.send_unack(
        ISSI,
        &unitdata(1, &datagram(Ipv4Addr::new(10, 0, 0, 99), GATEWAY, 9201, &wtp_get(1, "/status.wml"))),
    );
    air.send_unack(
        ISSI,
        &unitdata(1, &datagram(ip, Ipv4Addr::new(8, 8, 8, 8), 9201, &wtp_get(2, "/status.wml"))),
    );
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 80, &wtp_get(3, "/status.wml"))));
    air.send_unack(ISSI, &unitdata(2, &datagram(ip, GATEWAY, 9201, &wtp_get(4, "/status.wml")))); // no context on NSAPI 2
    air.run(100);
    assert!(air.take_sn().is_empty());
}

#[test]
fn wap_off_drops_datagrams_without_panic() {
    let mut air = Air::new(config(true, false));
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(1, "/"))));
    air.run(100);
    assert!(air.take_sn().is_empty());
}

#[test]
fn second_datagram_waits_until_the_first_left_the_mac() {
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    air.transmit = false;
    // The home page answers with a group of several datagrams at once.
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x30, "/"))));
    let first = air.next_sn(400).expect("first datagram");
    assert_eq!(first.sn_type(), Some(4));
    air.run(200);
    assert!(air.take_sn().is_empty(), "the MCCH holds one data PDU at a time");
    for reporter in air.held.drain(..) {
        if reporter.get_state() == TxState::Pending {
            reporter.mark_transmitted();
        }
    }
    air.transmit = true;
    let second = air.next_sn(8).expect("second datagram once the first went out");
    assert_eq!(second.sn_type(), Some(4));
}

#[test]
fn nothing_goes_down_to_a_radio_in_a_call() {
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    air.test.config.state_write().active_call_ts.insert(ISSI, (MAIN_CARRIER, 2, 4));
    air.run(80);
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x40, "/status.wml"))));
    air.run(300);
    assert!(air.take_sn().is_empty(), "radio on a traffic channel");
    air.test.config.state_write().active_call_ts.clear();
    let down = air.next_sn(200).expect("delivered after the call");
    assert_eq!(down.sn_type(), Some(4));
    // A group call of a group the radio is in holds it too.
    air.test.config.state_write().subscribers.register(ISSI);
    air.test.config.state_write().subscribers.affiliate(ISSI, 91);
    air.test.config.state_write().active_call_ts.insert(91, (MAIN_CARRIER, 3, 5));
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x41, "/status.wml"))));
    air.run(300);
    assert!(air.take_sn().is_empty(), "radio listening to its group");
    air.test.config.state_write().active_call_ts.clear();
    assert!(air.next_sn(200).is_some());
}

// ---------------------------------------------------------------------------------------------
// SN-DATA on the advanced link (scenarios of Nexus-BS test_sndcp_bs.rs, written for this harness)
// ---------------------------------------------------------------------------------------------

/// PDP context, SN-DATA TRANSMIT REQUEST (RESPONSE with no channel) and AL-SETUP, as the MXP600
/// does before its first WAP request.
fn attach_al(air: &mut Air, issi: u32) -> Ipv4Addr {
    let ip = air.attach(issi, 1);
    assert_eq!(air.al_setup(issi).setup_report, AlSetup::SETUP_REPORT_SUCCESS);
    air.take_sn();
    air.al_segments.clear();
    ip
}

/// Result of a WTP transaction the radio drives over SN-DATA: every datagram comes down as
/// SN-DATA on the advanced link, the radio acknowledges the groups and the last packet, and the
/// reassembled WTP payload is returned with the largest datagram size.
fn wtp_over_al(air: &mut Air, ip: Ipv4Addr, tid: u16) -> (Vec<u8>, usize, usize) {
    let mut parts: BTreeMap<u8, Vec<u8>> = BTreeMap::new();
    let (mut packets, mut largest) = (0, 0);
    loop {
        let down = air.next_sn(600).expect("the gateway's answer comes down");
        assert_eq!((down.issi, down.llc), (ISSI, LlcPduType::AlDataAlFinal), "SN-DATA on the AL");
        let (wtp, size) = wtp_down(&down, ip);
        packets += 1;
        largest = largest.max(size);
        let ty = (wtp[0] >> 3) & 0x0f;
        let flags = wtp[0] & 0x06;
        assert_eq!(u16::from_be_bytes([wtp[1], wtp[2]]), 0x8000 | tid);
        let (psn, data) = match ty {
            2 => (0, &wtp[3..]),
            6 => (wtp[3], &wtp[4..]),
            _ => panic!("WTP PDU type {ty}: {wtp:02x?}"),
        };
        parts.insert(psn, data.to_vec());
        if flags & 0x02 != 0 {
            let ack = wtp_ack(tid, (ty == 6).then_some(psn));
            air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &ack)));
            break;
        }
        if flags & 0x04 != 0 {
            air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(tid, Some(psn)))));
        }
    }
    (parts.into_values().flatten().collect(), packets, largest)
}

/// Nexus-BS `sndcp_wap_al_xhtml_e2e_...` scenario on the MCCH: DEMAND, TRANSMIT REQUEST, AL-SETUP,
/// a WSP GET of the home page in an AL-FINAL-AR, and the answer back as SN-DATA over the advanced
/// link in datagrams up to the MTU, each TL-SDU acknowledged by the radio.
#[test]
fn wsp_get_over_the_advanced_link_end_to_end() {
    debug::setup_logging_verbose();
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x21, "/"))));
    let (reply, _, largest) = wtp_over_al(&mut air, ip, 0x21);
    assert_eq!(&reply[..2], &[0x04, 0x20], "WSP Reply, 200 OK");
    let body = String::from_utf8_lossy(&reply);
    assert!(body.contains("FlowStation") && body.trim_end().ends_with("</html>"), "{body}");
    assert!(
        largest > 320 && largest <= 576,
        "datagrams up to the MTU on the AL, beyond the basic link's 320: {largest}"
    );
    assert!(air.al_segments.iter().all(|s| s.2 <= 194), "each segment fits one SCH/F");
    // The final Ack ends the transaction: nothing more comes down.
    air.run(200);
    assert!(air.take_sn().is_empty());
}

/// With N.271 = 256 octets the gateway answers in datagrams of at most 249 octets and splits the
/// page with WTP SAR.
#[test]
fn a_small_n271_limits_the_datagrams() {
    let mut air = Air::new(config(true, true));
    let ip = air.attach(ISSI, 1);
    let mut setup = radio_al_setup();
    setup.max_tl_sdu_len_code = 3;
    assert_eq!(air.al_setup_with(ISSI, setup).setup_report, AlSetup::SETUP_REPORT_SUCCESS);
    air.take_sn();
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x22, "/"))));
    let (reply, packets, largest) = wtp_over_al(&mut air, ip, 0x22);
    assert_eq!(&reply[..2], &[0x04, 0x20]);
    assert!(largest <= 249, "N.271 256 less FCS, SN-DATA header and discriminator: {largest}");
    assert!(packets > 1, "the page needs SAR");
}

/// The answer takes the bearer of the request, alternating on one radio: basic link for
/// SN-UNITDATA, advanced link for SN-DATA.
#[test]
fn answers_follow_the_bearer_of_the_request() {
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    for (tid, advanced) in [(0x31, false), (0x32, true), (0x33, false)] {
        let dg = datagram(ip, GATEWAY, 9201, &wtp_get(tid, "/status.wml"));
        if advanced {
            air.send_al(ISSI, &sn_data(1, &dg));
        } else {
            air.send_unack(ISSI, &unitdata(1, &dg));
        }
        let down = air.next_sn(400).expect("answer");
        let (wtp, _) = wtp_down(&down, ip);
        assert_eq!(u16::from_be_bytes([wtp[1], wtp[2]]), 0x8000 | tid);
        let expected = if advanced { LlcPduType::AlDataAlFinal } else { LlcPduType::BlUdata };
        assert_eq!(down.llc, expected, "TID {tid:#x}");
        let ack = datagram(ip, GATEWAY, 9201, &wtp_ack(tid, None));
        if advanced {
            air.send_al(ISSI, &sn_data(1, &ack));
        } else {
            air.send_unack(ISSI, &unitdata(1, &ack));
        }
        air.run(40);
        air.take_sn();
    }
}

/// Nexus-BS `sndcp_wap_al_connect_reply_e2e_...` scenario: a WSP Connect over the AL gets its
/// ConnectReply over the AL, acknowledged by the radio, and the session opens.
#[test]
fn wsp_connect_over_the_advanced_link() {
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_connect(0x60))));
    let reply = air.next_sn(400).expect("ConnectReply");
    assert_eq!(reply.llc, LlcPduType::AlDataAlFinal);
    let (wtp, _) = wtp_down(&reply, ip);
    assert_eq!(&wtp[..4], &[0x12, 0x80, 0x60, 0x02], "WTP Result with WSP ConnectReply");
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_ack(0x60, None))));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    air.run(20);
    let sessions = air
        .test
        .config
        .state_read()
        .wap_status
        .sessions
        .iter()
        .filter(|s| s.issi == ISSI)
        .count();
    assert_eq!(sessions, 1);
}

/// Nexus-BS WSP-over-UDP scenario: a request to port 9200 over the AL is answered from 9200 over
/// the AL.
#[test]
fn wsp_to_port_9200_over_the_advanced_link() {
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9200, &wtp_get(0x07, "/status.wml"))));
    let down = air.next_sn(400).expect("WSP Reply");
    assert_eq!(down.llc, LlcPduType::AlDataAlFinal);
    let unitdata = decode_sn_user_data_pdu(&down.sn_buf()).unwrap();
    let mut npdu = unitdata.n_pdu.clone();
    let mut octets = Vec::new();
    while let Some(b) = npdu.read_bits(8) {
        octets.push(b as u8);
    }
    let ipv4 = parse_ipv4_packet(&octets).unwrap();
    let udp = parse_udp_datagram(ipv4.payload).unwrap();
    assert_eq!((udp.source_port, udp.destination_port), (9200, RADIO_PORT));
    assert_eq!(u16::from_be_bytes([udp.payload[1], udp.payload[2]]), 0x8007, "WTP answer to TID 7");
}

/// SN-DATA on the advanced link without a PDP context, or on the basic link, goes nowhere (the
/// TL-SDU is still acknowledged by the LLC).
#[test]
fn sn_data_without_a_context_or_off_the_advanced_link_is_dropped() {
    let mut air = Air::new(config(true, true));
    air.al_setup(ISSI);
    air.send_al(
        ISSI,
        &sn_data(1, &datagram(Ipv4Addr::new(10, 0, 0, 2), GATEWAY, 9201, &wtp_get(1, "/"))),
    );
    air.run(200);
    assert!(air.take_sn().is_empty(), "no PDP context");
    let ip = air.attach(ISSI, 1);
    air.take_sn();
    air.send(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_get(2, "/"))));
    air.run(200);
    assert!(air.take_sn().is_empty(), "SN-DATA only on the advanced link (table 28.16)");
}

/// The radio does not acknowledge the TL-SDUs: the next datagram waits for the LLC to give up on
/// the first (at most one data PDU in the MAC for the MCCH), and the link is then closed.
#[test]
fn an_unacknowledged_sn_data_holds_the_next_until_the_llc_gives_up() {
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    air.al_answer = false;
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x41, "/"))));
    air.run(40);
    let first_ns: Vec<u8> = air.al_segments.iter().map(|s| s.1.ns).collect();
    assert!(
        !first_ns.is_empty() && first_ns.iter().all(|ns| *ns == 0),
        "only TL-SDU 0: {first_ns:?}"
    );
    air.run(2000);
    assert!(
        air.al_segments.iter().all(|s| s.1.ns == 0),
        "no second TL-SDU while the first is unacknowledged"
    );
    assert!(
        air.down.iter().any(|d| d.llc == LlcPduType::AlDisc),
        "the LLC closed the link after N.273"
    );
}

/// A radio that does not acknowledge its SN-DATA holds back only its own datagrams: the answer to
/// another radio goes down while the station still waits for the first one's acknowledgement.
#[test]
fn an_unacknowledged_sn_data_does_not_hold_other_radios() {
    let mut air = Air::new(config(true, true));
    let ip = attach_al(&mut air, ISSI);
    let ip2 = attach_al(&mut air, ISSI2);
    air.al_silent.push(ISSI);
    air.send_al(ISSI, &sn_data(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x42, "/"))));
    air.run(60);
    assert!(air.al_segments.iter().any(|s| s.0 == ISSI), "the first radio's answer is under way");
    air.send_al(ISSI2, &sn_data(1, &datagram(ip2, GATEWAY, 9201, &wtp_get(0x43, "/status.wml"))));
    let mut answered = false;
    for _ in 0..400 {
        air.step();
        assert!(
            !air.down.iter().any(|d| d.issi == ISSI && d.llc == LlcPduType::AlDisc),
            "the second radio waited until the LLC gave up on the first"
        );
        if air
            .down
            .iter()
            .any(|d| d.issi == ISSI2 && d.llc == LlcPduType::AlDataAlFinal && d.sn.is_some())
        {
            answered = true;
            break;
        }
    }
    assert!(answered, "the second radio got its answer");
}

// ---------------------------------------------------------------------------------------------
// Voice first: with the bearer on and no data, voice, SDS, registration and DGNA produce exactly
// the same messages as with it off
// ---------------------------------------------------------------------------------------------

const GSSI: u32 = 91;
const SECONDARY_CARRIER: u16 = 1522;

/// The stack from the LLC up (LLC, MLE, MM, CMCE, SNDCP), MAC and Brew as sinks, the radios'
/// PDUs coming in under the LLC. Every tick's output is kept as its sorted Debug lines (sorted:
/// entities keep calls in hash maps, whose order changes from run to run).
struct Voice {
    test: ComponentTest,
    dgna: tetra_entities::net_control::CommandDispatcher,
    ns: HashMap<u32, u8>,
    acks: Vec<(u32, u8, usize)>,
    ticks: usize,
    log: Vec<String>,
    msgs: Vec<SapMsg>,
    /// The real MAC under the LLC (its downlink blocks go in the log), not a sink.
    real_umac: bool,
}

/// The real UMAC, keeping a copy of what it is given.
struct TapUmac {
    inner: tetra_entities::umac::umac_bs::UmacBs,
    seen: Vec<SapMsg>,
}

impl tetra_entities::TetraEntityTrait for TapUmac {
    fn entity(&self) -> TetraEntity {
        TetraEntity::Umac
    }

    fn rx_prim(&mut self, queue: &mut tetra_entities::MessageQueue, message: SapMsg) {
        self.seen.push(message.clone());
        self.inner.rx_prim(queue, message);
    }

    fn set_config(&mut self, config: SharedConfig) {
        self.inner.set_config(config);
    }

    fn tick_start(&mut self, queue: &mut tetra_entities::MessageQueue, ts: TdmaTime) {
        self.inner.tick_start(queue, ts);
    }

    fn tick_end(&mut self, queue: &mut tetra_entities::MessageQueue, ts: TdmaTime) -> bool {
        self.inner.tick_end(queue, ts)
    }
}

/// Configuration for the voice scenarios: packet data off, or on with `bearer`.
fn voice_config(bearer: Option<PacketDataBearer>, secondary: bool) -> StackConfig {
    let mut cfg = config(bearer.is_some(), bearer.is_some());
    if let Some(bearer) = bearer {
        cfg.packet_data.bearer = bearer;
    }
    if secondary {
        cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    }
    cfg
}

impl Voice {
    fn new(cfg: StackConfig, real_umac: bool) -> Self {
        Self::with_telemetry(cfg, real_umac, None)
    }

    /// With the real MAC, `telemetry` is the UMAC's telemetry sink.
    fn with_telemetry(cfg: StackConfig, real_umac: bool, telemetry: Option<tetra_entities::net_telemetry::TelemetrySink>) -> Self {
        let mut test = ComponentTest::from_config(cfg, Some(TdmaTime::default()));
        test.populate_entities(
            vec![TetraEntity::Llc, TetraEntity::Mle, TetraEntity::Cmce, TetraEntity::Sndcp],
            if real_umac {
                vec![TetraEntity::Lmac, TetraEntity::Brew]
            } else {
                vec![TetraEntity::Umac, TetraEntity::Brew]
            },
        );
        if real_umac {
            let inner = tetra_entities::umac::umac_bs::UmacBs::new(test.get_shared_config(), telemetry);
            test.register_entity(TapUmac { inner, seen: Vec::new() });
        }
        let (dgna, endpoint) = tetra_entities::net_control::make_control_link();
        let mm = tetra_entities::mm::mm_bs::MmBs::new(test.get_shared_config(), None, Some(endpoint));
        test.register_entity(mm);
        Self {
            test,
            dgna,
            ns: HashMap::new(),
            acks: Vec::new(),
            ticks: 0,
            log: Vec::new(),
            msgs: Vec::new(),
            real_umac,
        }
    }

    /// What the UMAC was given since the last call (real MAC only).
    fn take_tapped(&mut self) -> Vec<SapMsg> {
        self.test
            .router
            .get_entity(TetraEntity::Umac)
            .and_then(|e| e.as_any_mut().downcast_mut::<TapUmac>())
            .map(|tap| std::mem::take(&mut tap.seen))
            .unwrap_or_default()
    }

    /// Run `ticks` ticks; the radios behave like the air: every PDU is reported transmitted and
    /// each BL-DATA to a radio is acknowledged two slots later.
    fn run(&mut self, ticks: usize) {
        for _ in 0..ticks {
            let due: Vec<(u32, u8)> = self.acks.iter().filter(|a| a.2 <= self.ticks).map(|a| (a.0, a.1)).collect();
            self.acks.retain(|a| a.2 > self.ticks);
            for (issi, nr) in due {
                let mut pdu = BitBuffer::new_autoexpand(8);
                BlAck { has_fcs: false, nr }.to_bitbuf(&mut pdu);
                pdu.seek(0);
                self.test.submit_message(tma_ind(issi, pdu, 1));
            }
            self.test.run_stack(Some(1));
            self.ticks += 1;
            let mut msgs = self.test.dump_sinks();
            msgs.extend(self.take_tapped());
            let mut lines: Vec<String> = msgs.iter().map(|m| format!("{m:?}")).collect();
            lines.sort();
            for line in lines {
                self.log.push(format!("{} {line}", self.ticks));
            }
            for m in &msgs {
                let SapMsgInner::TmaUnitdataReq(req) = &m.msg else { continue };
                // The real MAC reports its own transmissions.
                if !self.real_umac
                    && let Some(reporter) = &req.tx_reporter
                    && reporter.get_state() == TxState::Pending
                {
                    reporter.mark_transmitted();
                }
                if req.main_address.ssi_type != SsiType::Gssi
                    && let Some(ns) = down_ns(&req.pdu)
                {
                    self.acks.push((req.main_address.ssi, ns, self.ticks + 1));
                }
            }
            self.msgs.extend(msgs);
        }
    }

    /// A PDU of protocol `pd` from `issi` on the MCCH (BL-DATA, uplink slot TS1).
    fn uplink(&mut self, issi: u32, pd: MleProtocolDiscriminator, sdu: BitBuffer) {
        while self.ticks % 4 != 2 {
            self.run(1);
        }
        let ns = self.ns.entry(issi).or_insert(0);
        let mut pdu = BitBuffer::new_autoexpand(96);
        BlData { has_fcs: false, ns: *ns }.to_bitbuf(&mut pdu);
        *ns ^= 1;
        pdu.write_bits(pd.into_raw(), 3);
        let mut sdu = BitBuffer::from_bitbuffer(&sdu);
        let len = sdu.get_len();
        pdu.copy_bits(&mut sdu, len);
        pdu.seek(0);
        self.test.submit_message(tma_ind(issi, pdu, 1));
        self.run(1);
    }

    fn cmce(&mut self, issi: u32, sdu: BitBuffer) {
        self.uplink(issi, MleProtocolDiscriminator::Cmce, sdu);
    }

    /// Register `issi` and affiliate it to `gssi` in CMCE, as the MM does.
    fn register(&mut self, issi: u32, gssi: Option<u32>) {
        use tetra_saps::control::brew::{BrewSubscriberAction, MmSubscriberUpdate};
        let mut update = |groups: Vec<u32>, action| {
            self.test.submit_message(SapMsg {
                sap: Sap::Control,
                src: TetraEntity::Mm,
                dest: TetraEntity::Cmce,
                msg: SapMsgInner::MmSubscriberUpdate(MmSubscriberUpdate { issi, groups, action }),
            });
        };
        update(Vec::new(), BrewSubscriberAction::Register);
        if let Some(gssi) = gssi {
            update(vec![gssi], BrewSubscriberAction::Affiliate);
        }
        let mut state = self.test.config.state_write();
        state.subscribers.register(issi);
        if let Some(gssi) = gssi {
            state.subscribers.affiliate(issi, gssi);
        }
        drop(state);
        self.run(1);
    }

    /// Call identifier of the last D-SETUP that went down to `ssi`.
    fn d_setup_call_id(&self, ssi: u32) -> u16 {
        use tetra_pdus::cmce::enums::cmce_pdu_type_dl::CmcePduTypeDl;
        use tetra_pdus::cmce::pdus::d_setup::DSetup;
        self.msgs
            .iter()
            .rev()
            .find_map(|m| {
                let SapMsgInner::TmaUnitdataReq(req) = &m.msg else { return None };
                if req.main_address.ssi != ssi {
                    return None;
                }
                let mut pdu = req.pdu.clone();
                match LlcPduType::try_from(pdu.peek_bits(4)?).ok()? {
                    LlcPduType::BlData => BlData::from_bitbuf(&mut pdu).ok().map(|_| ())?,
                    LlcPduType::BlAdata => BlAdata::from_bitbuf(&mut pdu).ok().map(|_| ())?,
                    LlcPduType::BlUdata => BlUdata::from_bitbuf(&mut pdu).ok().map(|_| ())?,
                    _ => return None,
                }
                if pdu.read_bits(3)? != MleProtocolDiscriminator::Cmce.into_raw() {
                    return None;
                }
                if CmcePduTypeDl::try_from(pdu.peek_bits(5)?).ok()? != CmcePduTypeDl::DSetup {
                    return None;
                }
                DSetup::from_bitbuf(&mut pdu).ok().map(|d| d.call_identifier)
            })
            .expect("a D-SETUP went down")
    }
}

fn u_setup(called_ssi: u32, group: bool, duplex: bool, call_priority: u8) -> BitBuffer {
    use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
    use tetra_pdus::cmce::fields::basic_service_information::BasicServiceInformation;
    use tetra_pdus::cmce::pdus::u_setup::USetup;
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
    use tetra_saps::control::enums::communication_type::CommunicationType;
    let u_setup = USetup {
        area_selection: 0,
        hook_method_selection: !group,
        simplex_duplex_selection: duplex,
        basic_service_information: BasicServiceInformation {
            circuit_mode_type: CircuitModeType::TchS,
            encryption_flag: false,
            communication_type: if group { CommunicationType::P2Mp } else { CommunicationType::P2p },
            slots_per_frame: None,
            speech_service: Some(0),
        },
        request_to_transmit_send_data: false,
        call_priority,
        clir_control: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_ssi: Some(called_ssi as u64),
        called_party_short_number_address: None,
        called_party_extension: None,
        external_subscriber_number: None,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    };
    let mut sdu = BitBuffer::new_autoexpand(80);
    u_setup.to_bitbuf(&mut sdu).expect("Failed to serialize USetup");
    sdu.seek(0);
    sdu
}

fn u_connect(call_id: u16, duplex: bool) -> BitBuffer {
    use tetra_pdus::cmce::pdus::u_connect::UConnect;
    let mut sdu = BitBuffer::new_autoexpand(80);
    UConnect {
        call_identifier: call_id,
        hook_method_selection: true,
        simplex_duplex_selection: duplex,
        basic_service_information: None,
        facility: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("Failed to serialize UConnect");
    sdu.seek(0);
    sdu
}

fn u_tx_ceased(call_id: u16) -> BitBuffer {
    use tetra_pdus::cmce::pdus::u_tx_ceased::UTxCeased;
    let mut sdu = BitBuffer::new_autoexpand(80);
    UTxCeased {
        call_identifier: call_id,
        facility: None,
        dm_ms_address: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("Failed to serialize UTxCeased");
    sdu.seek(0);
    sdu
}

fn u_disconnect(call_id: u16) -> BitBuffer {
    use tetra_pdus::cmce::enums::disconnect_cause::DisconnectCause;
    use tetra_pdus::cmce::pdus::u_disconnect::UDisconnect;
    let mut sdu = BitBuffer::new_autoexpand(80);
    UDisconnect {
        call_identifier: call_id,
        disconnect_cause: DisconnectCause::UserRequestedDisconnection,
        facility: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("Failed to serialize UDisconnect");
    sdu.seek(0);
    sdu
}

fn u_sds_data(dest_ssi: u32, payload: u16) -> BitBuffer {
    use tetra_pdus::cmce::enums::party_type_identifier::PartyTypeIdentifier;
    use tetra_pdus::cmce::pdus::u_sds_data::USdsData;
    use tetra_saps::control::enums::sds_user_data::SdsUserData;
    let mut sdu = BitBuffer::new_autoexpand(80);
    USdsData {
        area_selection: 0,
        called_party_type_identifier: PartyTypeIdentifier::Ssi,
        called_party_short_number_address: None,
        called_party_ssi: Some(dest_ssi as u64),
        called_party_extension: None,
        user_defined_data: SdsUserData::Type1(payload),
        external_subscriber_number: None,
        dm_ms_address: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("Failed to serialize U-SDS-DATA");
    sdu.seek(0);
    sdu
}

fn u_location_update_demand(issi: u32) -> BitBuffer {
    use tetra_pdus::mm::enums::location_update_type::LocationUpdateType;
    use tetra_pdus::mm::pdus::u_location_update_demand::ULocationUpdateDemand;
    let mut sdu = BitBuffer::new_autoexpand(32);
    ULocationUpdateDemand {
        location_update_type: LocationUpdateType::RoamingLocationUpdating,
        request_to_append_la: false,
        cipher_control: false,
        ciphering_parameters: None,
        class_of_ms: None,
        energy_saving_mode: None,
        la_information: None,
        ssi: Some(issi as u64),
        address_extension: None,
        group_identity_location_demand: None,
        group_report_response: None,
        authentication_uplink: None,
        extended_capabilities: None,
        proprietary: None,
    }
    .to_bitbuf(&mut sdu)
    .expect("serialize U-LOCATION-UPDATE-DEMAND");
    sdu.seek(0);
    sdu
}

/// Run `scenario` with the bearer off and on, on the MCCH and on a PDCH (no data traffic), and
/// require the same output; with the PDCH bearer also through the real MAC, down to the blocks
/// on the air.
fn same_with_and_without_packet_data(secondary: bool, scenario: impl Fn(&mut Voice)) -> Vec<String> {
    let run = |bearer: Option<PacketDataBearer>, real_umac: bool| {
        let mut v = Voice::new(voice_config(bearer, secondary), real_umac);
        scenario(&mut v);
        v.log
    };
    let same = |off: &[String], on: &[String], what: &str| {
        assert_eq!(off.len(), on.len(), "as many messages with {what}");
        for (a, b) in off.iter().zip(on) {
            assert_eq!(a, b, "first difference with {what}");
        }
    };
    let off = run(None, false);
    same(&off, &run(Some(PacketDataBearer::Mcch), false), "[packet_data] on the MCCH");
    same(&off, &run(Some(PacketDataBearer::Pdch), false), "[packet_data] on a PDCH");
    same(
        &run(None, true),
        &run(Some(PacketDataBearer::Pdch), true),
        "[packet_data] on a PDCH, through the real MAC",
    );
    off
}

fn count(log: &[String], what: &str) -> usize {
    log.iter().filter(|l| l.contains(what)).count()
}

#[test]
fn group_call_is_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        v.register(ISSI, Some(GSSI));
        v.register(ISSI2, Some(GSSI));
        v.cmce(ISSI, u_setup(GSSI, true, false, 0));
        v.run(20);
        let call_id = v.d_setup_call_id(GSSI);
        v.cmce(ISSI, u_tx_ceased(call_id));
        // Hangtime (5 s) and the D-RELEASE.
        v.run(450);
    });
    assert!(count(&log, "Open(") >= 1, "the group call opened a circuit");
    assert!(count(&log, "CallEnded") >= 1, "and ended after the hangtime");
}

#[test]
fn simplex_individual_call_is_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        v.register(ISSI2, None);
        v.cmce(ISSI, u_setup(ISSI2, false, false, 0));
        v.run(10);
        let call_id = v.d_setup_call_id(ISSI2);
        v.cmce(ISSI2, u_connect(call_id, false));
        v.run(10);
        v.cmce(ISSI, u_tx_ceased(call_id));
        v.run(10);
        v.cmce(ISSI, u_disconnect(call_id));
        v.run(60);
    });
    assert!(count(&log, "Open(") >= 1);
}

#[test]
fn duplex_individual_calls_are_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        v.register(ISSI2, None);
        v.cmce(ISSI, u_setup(ISSI2, false, true, 0));
        v.run(10);
        let call_id = v.d_setup_call_id(ISSI2);
        v.cmce(ISSI2, u_connect(call_id, true));
        v.run(10);
        v.cmce(ISSI, u_disconnect(call_id));
        v.run(60);
    });
    assert!(count(&log, "Open(") >= 2, "a duplex call opens two circuits");
}

/// Two duplex calls need four slots: the second one goes to the secondary carrier.
#[test]
fn duplex_calls_on_two_carriers_are_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(true, |v| {
        v.register(ISSI2, None);
        v.register(ISSI2 + 2, None);
        v.cmce(ISSI, u_setup(ISSI2, false, true, 0));
        v.run(10);
        let first = v.d_setup_call_id(ISSI2);
        v.cmce(ISSI2, u_connect(first, true));
        v.run(10);
        v.cmce(ISSI + 2, u_setup(ISSI2 + 2, false, true, 0));
        v.run(10);
        let second = v.d_setup_call_id(ISSI2 + 2);
        v.cmce(ISSI2 + 2, u_connect(second, true));
        v.run(10);
        v.cmce(ISSI, u_disconnect(first));
        v.cmce(ISSI + 2, u_disconnect(second));
        v.run(60);
    });
    assert!(count(&log, "Open(") >= 4);
    assert!(count(&log, "carrier_num: 1522") >= 1, "the second call on the secondary carrier");
}

#[test]
fn local_sds_is_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        v.register(ISSI2, None);
        v.cmce(ISSI, u_sds_data(ISSI2, 0xABCD));
        v.run(20);
    });
    assert!(
        log.iter()
            .any(|l| l.contains("TmaUnitdataReq") && l.contains(&format!("ssi: {ISSI2}")))
    );
}

#[test]
fn registration_and_dgna_are_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        v.uplink(ISSI, MleProtocolDiscriminator::Mm, u_location_update_demand(ISSI));
        v.run(20);
        v.dgna.send(tetra_entities::net_control::ControlCommand::Dgna {
            issi: ISSI,
            gssi: 100,
            mnemonic: None,
            attachment_mode: 0,
            attach: true,
        });
        v.run(40);
    });
    assert!(
        count(&log, &format!("ssi: {ISSI}")) >= 2,
        "D-LOCATION UPDATE ACCEPT and the DGNA went down"
    );
}

#[test]
fn emergency_call_in_a_full_cell_is_the_same_with_packet_data_on() {
    let log = same_with_and_without_packet_data(false, |v| {
        for (i, g) in [101u32, 102, 103, 199].into_iter().enumerate() {
            v.register(2_000_001 + i as u32, Some(g));
        }
        for (i, g) in [101u32, 102, 103].into_iter().enumerate() {
            v.cmce(3_000_001 + i as u32, u_setup(g, true, false, 0));
            v.run(4);
        }
        v.cmce(3_000_099, u_setup(199, true, false, 15));
        v.run(20);
    });
    assert!(count(&log, "CallEnded") >= 1, "the emergency call pre-empted one");
}

// ---------------------------------------------------------------------------------------------
// bearer = "pdch": a packet-data channel of one main-carrier slot, which voice takes back
// ---------------------------------------------------------------------------------------------

fn pdch_config(wap: bool) -> StackConfig {
    let mut cfg = config(true, wap);
    cfg.packet_data.bearer = PacketDataBearer::Pdch;
    cfg
}

/// Give `issi` the packet-data channel on `ts` in the shared state, as the SNDCP runtime does.
fn grant_pdch(config: &SharedConfig, issi: u32, ts: u8, on_air: bool) -> CarrierSlot {
    let mut state = config.state_write();
    let slot = state.timeslot_alloc.reserve_packet_data_slot(&[ts]).expect("slot free");
    state.pdch_by_issi.insert(issi, PdchGrant { slot, on_air });
    slot
}

/// Voice fills main-carrier ts2 and ts3, then takes the packet-data slot (single carrier).
fn voice_takes_the_pdch(config: &SharedConfig) -> CarrierSlot {
    let mut state = config.state_write();
    state.timeslot_alloc.reserve(TimeslotOwner::Cmce, 2).unwrap();
    state.timeslot_alloc.reserve(TimeslotOwner::Cmce, 3).unwrap();
    state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("the PDCH slot")
}

/// The AACH of a downlink slot: (header, ACCESS-ASSIGN).
fn aach_of(slot: &tetra_saps::tmv::TmvUnitdataReqSlot) -> (u8, tetra_pdus::umac::pdus::access_assign::AccessAssign) {
    let mut bbk = slot.bbk.as_ref().expect("AACH").mac_block.clone();
    bbk.seek(0);
    let header = bbk.peek_bits(2).unwrap() as u8;
    (
        header,
        tetra_pdus::umac::pdus::access_assign::AccessAssign::from_bitbuf(&mut bbk).unwrap(),
    )
}

/// The first MAC-RESOURCE of a slot's first block, if it is one.
fn first_resource(slot: &tetra_saps::tmv::TmvUnitdataReqSlot) -> Option<tetra_pdus::umac::pdus::mac_resource::MacResource> {
    let blk = slot.blk1.as_ref()?;
    let mut b = blk.mac_block.clone();
    b.seek(0);
    if b.peek_bits(2) != Some(0) {
        return None;
    }
    tetra_pdus::umac::pdus::mac_resource::MacResource::from_bitbuf(&mut b)
        .ok()
        .filter(|r| r.addr.is_some())
}

fn tl_data_to(addr: TetraAddress, follow_uplink_channel: bool) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Mle,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlDataReqBl(tetra_saps::tla::TlaTlDataReqBl {
            main_address: addr,
            link_id: 0,
            endpoint_id: 0,
            tl_sdu: BitBuffer::from_bitstr("0101100110011"),
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag: false,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_class_info: None,
            req_handle: 0,
            graceful_degradation: None,
            chan_alloc: None,
            follow_uplink_channel,
            tx_reporter: None,
        }),
    }
}

fn tl_unitdata_to(addr: TetraAddress, octets: usize, reporter: Option<TxReporter>) -> SapMsg {
    SapMsg {
        sap: Sap::TlaSap,
        src: TetraEntity::Sndcp,
        dest: TetraEntity::Llc,
        msg: SapMsgInner::TlaTlUnitdataReqBl(tetra_saps::tla::TlaTlUnitdataReqBl {
            main_address: addr,
            link_id: 0,
            endpoint_id: 0,
            tl_sdu: BitBuffer::from_bytes(&vec![0x5a; octets]),
            stealing_permission: false,
            subscriber_class: 0,
            fcs_flag: false,
            air_interface_encryption: None,
            packet_data_flag: true,
            n_tlsdu_repeats: 0,
            data_class_info: None,
            req_handle: 0,
            chan_alloc: None,
            tx_reporter: reporter,
        }),
    }
}

fn ts_of(alloc: &CmceChanAllocReq) -> Vec<u8> {
    (1..=4u8).filter(|ts| alloc.timeslots[*ts as usize - 1]).collect()
}

// The MAC -------------------------------------------------------------------------------------

/// A PDCH grant turns the slot into an assigned control channel for its radio: AACH assigned
/// control / assigned only, SCH/F Null while idle, the radio's PDUs there without the random
/// access flag; without the grant the slot is unallocated again.
#[test]
fn a_pdch_grant_makes_the_slot_an_assigned_channel() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(pdch_config(false));
    let before = mac.run_slots(8, 4);
    assert!(
        before
            .iter()
            .all(|s| aach_of(s).1.dl_usage == tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage::Unallocated)
    );
    let slot = grant_pdch(&mac.test.config, ISSI, 4, true);
    let idle = mac.run_slots(8, 4);
    assert!(!idle.is_empty());
    for s in &idle[1..] {
        let (header, aach) = aach_of(s);
        assert_eq!(header, 2);
        assert_eq!(
            (aach.dl_usage, aach.ul_usage),
            (
                tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage::AssignedControl,
                tetra_pdus::umac::enums::access_assign_ul_usage::AccessAssignUlUsage::AssignedOnly
            )
        );
        assert_eq!(
            s.blk1.as_ref().unwrap().logical_channel,
            tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF
        );
    }
    mac.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    let with_pdu = mac.run_slots(12, 4);
    let res = with_pdu.iter().find_map(first_resource).expect("the TL-DATA on the PDCH");
    assert_eq!(res.addr.unwrap().ssi, ISSI);
    assert!(!res.random_access_flag, "not a random access answer");
    // Released: back to an unallocated slot with SYNC.
    {
        let mut state = mac.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, slot).unwrap();
    }
    let after = mac.run_slots(8, 4);
    let last = after.last().unwrap();
    assert_eq!(
        aach_of(last).1.dl_usage,
        tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage::Unallocated
    );
    assert_eq!(
        last.blk1.as_ref().unwrap().logical_channel,
        tetra_saps::tmv::enums::logical_chans::LogicalChannel::Bsch
    );
}

fn mac_u_blck(event_label: u16, reservation_req: u8) -> SapMsg {
    mac_u_blck_with(event_label, reservation_req, &BitBuffer::from_bitstr("10101100"))
}

/// MAC-U-BLCK carrying the TM-SDU `sdu`.
fn mac_u_blck_with(event_label: u16, reservation_req: u8, sdu: &BitBuffer) -> SapMsg {
    let mut pdu = BitBuffer::new(268);
    tetra_pdus::umac::pdus::mac_u_blck::MacUBlck {
        fill_bits: true,
        encrypted: false,
        event_label,
        reservation_req,
    }
    .to_bitbuf(&mut pdu);
    let mut sdu = sdu.clone();
    sdu.seek(0);
    let len = sdu.get_len();
    pdu.copy_bits(&mut sdu, len);
    pdu.write_bits(1, 1); // fill bits: a one, then zeros
    pdu.seek(0);
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(tetra_saps::tmv::TmvUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu,
            block_num: tetra_core::PhyBlockNum::Both,
            logical_channel: tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    }
}

/// MAC-U-BLCK on ts4 received in the tick whose downlink slot is ts2: what reaches the LLC and
/// the downlink slots of ts4 afterwards.
fn mac_u_blck_on_ts4(cfg: StackConfig, grant: bool, reservation_req: u8) -> (Vec<SapMsg>, Vec<tetra_saps::tmv::TmvUnitdataReqSlot>) {
    let mut test = ComponentTest::from_config(cfg, Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc, TetraEntity::Lmac]);
    if grant {
        grant_pdch(&test.config, ISSI, 4, true);
    }
    test.run_stack(Some(1));
    test.dump_sinks();
    test.submit_message(mac_u_blck(5, reservation_req));
    let mut up = Vec::new();
    let mut ts4 = Vec::new();
    for _ in 0..16 {
        test.run_stack(Some(1));
        for msg in test.dump_sinks() {
            match msg.msg {
                SapMsgInner::TmaUnitdataInd(_) => up.push(msg),
                SapMsgInner::TmvUnitdataReqSlots(slots) => ts4.extend(slots.slots.into_iter().filter(|s| s.ts.t == 4)),
                _ => {}
            }
        }
    }
    (up, ts4)
}

/// MAC-U-BLCK on the PDCH belongs to its radio (this BS assigns no event labels): the TM-SDU
/// reaches the LLC with the timeslot, and a reservation is granted on the PDCH. Anywhere else, or
/// with the bearer on the MCCH, it is dropped as before.
#[test]
fn mac_u_blck_on_the_pdch_reaches_the_llc() {
    debug::setup_logging_verbose();
    let (up, _) = mac_u_blck_on_ts4(pdch_config(false), true, 15);
    assert_eq!(up.len(), 1, "{up:?}");
    let SapMsgInner::TmaUnitdataInd(ind) = &up[0].msg else {
        unreachable!()
    };
    assert_eq!((ind.main_address.ssi, ind.link_id), (ISSI, 4));
    assert_eq!(ind.pdu.as_ref().unwrap().to_bitstr(), "10101100");

    let (up, ts4) = mac_u_blck_on_ts4(pdch_config(false), true, 2);
    assert_eq!(up.len(), 1);
    let grant = ts4
        .iter()
        .filter_map(first_resource)
        .find(|r| r.addr.is_some_and(|a| a.ssi == ISSI))
        .expect("an answer on the PDCH");
    assert!(grant.slot_granting_element.is_some(), "the reservation granted on the PDCH");

    let (up, _) = mac_u_blck_on_ts4(pdch_config(false), false, 15);
    assert!(up.is_empty(), "not a PDCH: dropped");
    let (up, _) = mac_u_blck_on_ts4(config(true, false), true, 15);
    assert!(up.is_empty(), "bearer on the MCCH: dropped as before");
}

/// Voice takes the PDCH slot: the next block of that slot is the call's (AACH traffic), and what
/// was still queued there for the data radio is dropped; its next PDU goes on the MCCH.
#[test]
fn voice_taking_the_pdch_slot_shows_traffic_and_drops_its_data() {
    use tetra_core::Direction;
    use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(pdch_config(false));
    grant_pdch(&mac.test.config, ISSI, 4, true);
    mac.run_slots(4, 4);
    let reporter = TxReporter::new_unacked();
    mac.test
        .submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 120, Some(reporter.clone())));
    let first = mac.run_slots(8, 4);
    assert!(
        first
            .iter()
            .filter_map(first_resource)
            .any(|r| r.addr.is_some_and(|a| a.ssi == ISSI))
    );
    assert_eq!(reporter.get_state(), TxState::Pending, "more fragments to go");

    let slot = voice_takes_the_pdch(&mac.test.config);
    assert_eq!(slot.ts, 4);
    mac.test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            carrier_num: MAIN_CARRIER,
            ts: 4,
            peer_carrier_num: None,
            peer_ts: None,
            usage: 6,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::SwMI,
        })),
    });
    let after = mac.run_slots(8, 4);
    assert_eq!(reporter.get_state(), TxState::Discarded);
    for s in &after[1..] {
        assert_eq!(
            aach_of(s).1.dl_usage,
            tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage::Traffic(6)
        );
        assert_eq!(
            s.blk1.as_ref().unwrap().logical_channel,
            tetra_saps::tmv::enums::logical_chans::LogicalChannel::TchS
        );
    }
    mac.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    let blocks = mac.run(12);
    let mine: Vec<_> = blocks.iter().filter(|b| b.3.iter().any(|r| r.0 == ISSI)).collect();
    assert!(
        !mine.is_empty() && mine.iter().all(|b| (b.1, b.2) == (MAIN_CARRIER, 1)),
        "back on the MCCH: {mine:?}"
    );
}

/// The advanced link of a radio on its PDCH (the MXP600's path): every
/// segment goes on the PDCH, whole in one block each.
#[test]
fn the_advanced_link_of_a_radio_on_its_pdch_runs_there() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(pdch_config(false));
    grant_pdch(&mac.test.config, ISSI, 4, true);
    let reporter = mac.transfer(300);
    let blocks = mac.run(4 * 20);
    let segments = al_segments_on_air(&blocks);
    assert_eq!(segments.len(), 13, "{segments:?}");
    for (carrier, ts, length_ind) in &segments {
        assert_eq!((*carrier, *ts), (MAIN_CARRIER, 4), "on the PDCH");
        assert_ne!(*length_ind, 0b111111, "never fragmented by the MAC");
    }
    assert_eq!(reporter.get_state(), TxState::Transmitted);
}

// The LLC -------------------------------------------------------------------------------------

/// A radio on its PDCH gets its PDUs there (acknowledged and unacknowledged basic link, MM
/// following the uplink, the BL-ACK of an uplink on the PDCH), never stolen; groups, radios
/// without a PDCH and a PDCH voice took go the way they always went.
#[test]
fn pdus_for_a_radio_on_its_pdch_go_on_that_slot() {
    debug::setup_logging_verbose();
    let mut air = Air::new(pdch_config(false));
    grant_pdch(&air.test.config, ISSI, 4, true);
    let down_to = |air: &mut Air, issi: u32| -> Down {
        air.run(3);
        let i = air.down.iter().position(|d| d.issi == issi).expect("a PDU went down");
        air.down.remove(i)
    };
    for follow in [false, true] {
        air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), follow));
        let d = down_to(&mut air, ISSI);
        assert_eq!((d.link_id, d.stealing, d.chan_alloc), (4, false, false), "TL-DATA follow={follow}");
    }
    // MM follows the uplink to a traffic slot, but a radio on its PDCH is on its PDCH.
    let mut udata = BitBuffer::new_autoexpand(32);
    BlUdata { has_fcs: false }.to_bitbuf(&mut udata);
    append_bits(&mut udata, "0101010101");
    air.uplink_on(ISSI, udata, 3);
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), true));
    assert_eq!(down_to(&mut air, ISSI).link_id, 4);

    air.test.submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 10, None));
    let d = down_to(&mut air, ISSI);
    assert_eq!((d.llc, d.link_id, d.stealing), (LlcPduType::BlUdata, 4, false));

    // A BL-DATA from the radio on its PDCH is acknowledged there.
    air.down.clear();
    let mut data = BitBuffer::new_autoexpand(32);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut data);
    append_bits(&mut data, "0101010101");
    air.uplink_on(ISSI, data, 4);
    air.run(2);
    let ack = air
        .down
        .iter()
        .find(|d| d.issi == ISSI && d.llc == LlcPduType::BlAck)
        .expect("BL-ACK");
    assert_eq!((ack.link_id, ack.stealing, ack.chan_alloc), (4, false, false));

    // A group and a radio without a PDCH: MCCH.
    air.test.submit_message(tl_data_to(TetraAddress::new(GSSI, SsiType::Gssi), false));
    assert_eq!(down_to(&mut air, GSSI).link_id, 0);
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI2), false));
    assert_eq!(down_to(&mut air, ISSI2).link_id, 0);

    // Voice took the slot: MCCH again.
    voice_takes_the_pdch(&air.test.config);
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    let d = down_to(&mut air, ISSI);
    assert_eq!((d.link_id, d.stealing), (0, false));
}

/// PDUs for a radio queued while it was on its PDCH (waiting behind another, or retransmitted)
/// go on the MCCH once the PDCH is gone: the route is taken when they go down, not when queued.
#[test]
fn queued_pdus_follow_the_radio_off_its_pdch() {
    let mut air = Air::new(pdch_config(false));
    let slot = grant_pdch(&air.test.config, ISSI, 4, true);
    // Two TL-DATA; the radio never acknowledges: the second waits for the first to be given up.
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.step();
    air.acks.clear();
    let first: Vec<u32> = air.down.drain(..).filter(|d| d.issi == ISSI).map(|d| d.link_id).collect();
    assert_eq!(first, vec![4], "the first on the PDCH");
    {
        let mut state = air.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, slot).unwrap();
    }
    for _ in 0..200 {
        air.step();
        air.acks.clear();
    }
    let links: Vec<u32> = air.down.iter().filter(|d| d.issi == ISSI).map(|d| d.link_id).collect();
    assert!(
        links.len() >= 4 && links.iter().all(|l| *l == 0),
        "retransmissions of the first and the second on the MCCH: {links:?}"
    );
}

/// A radio that went from its PDCH into a call: an MM PDU following its uplink goes on the
/// call's slot, not on the PDCH (which the station gives back within a second).
#[test]
fn mm_follows_a_radio_from_its_pdch_into_its_call() {
    let mut air = Air::new(pdch_config(false));
    grant_pdch(&air.test.config, ISSI, 4, true);
    air.test.config.state_write().active_call_ts.insert(ISSI, (MAIN_CARRIER, 3, 4));
    let mut udata = BitBuffer::new_autoexpand(32);
    BlUdata { has_fcs: false }.to_bitbuf(&mut udata);
    append_bits(&mut udata, "0101010101");
    air.uplink_on(ISSI, udata, 3);
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), true));
    air.run(3);
    let d = air.down.iter().find(|d| d.issi == ISSI).expect("a PDU went down");
    assert_eq!((d.link_id, d.stealing), (3, true), "on the call's slot");
}

// The SNDCP runtime ---------------------------------------------------------------------------

/// PDP context and TRANSMIT REQUEST: the RESPONSE assigns the PDCH (sent on the MCCH), which the
/// radio is on once it went out. Returns the radio's address.
fn onto_pdch(air: &mut Air, issi: u32) -> Ipv4Addr {
    air.send(issi, &demand(1, None, false));
    let ip = accept_ip(&air.next_sn(8).expect("ACCEPT"));
    air.send(issi, &transmit_request(1, Some((1, false))));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    assert!(response.alloc.is_some(), "PDCH assigned");
    assert_eq!(response.category, Some(DATA_CATEGORY_PDCH_ASSIGNMENT), "marked as such for the MAC");
    air.run(2);
    assert!(air.test.config.state_read().pdch_by_issi.get(&issi).is_some_and(|g| g.on_air));
    air.take_sn();
    ip
}

fn pdch_slot_of(air: &Air, issi: u32) -> Option<u8> {
    air.test.config.state_read().pdch_by_issi.get(&issi).map(|g| g.slot.ts)
}

/// The MXP600's TRANSMIT REQUEST (one slot, or four) gets one PDCH slot of the main carrier,
/// both directions, replacing the MCCH; the radio's datagrams and the answers then go on it.
#[test]
fn transmit_request_assigns_a_pdch_and_the_data_goes_on_it() {
    debug::setup_logging_verbose();
    for slots in [Some((1, false)), Some((4, true)), None] {
        let mut air = Air::new(pdch_config(true));
        air.send(ISSI, &demand(1, None, true));
        let ip = accept_ip(&air.next_sn(8).unwrap());
        air.send(ISSI, &transmit_request(1, slots));
        let response = air.next_sn(8).unwrap();
        assert_eq!(
            decode_data_transmit_response(&response.sn_buf()).unwrap().result,
            SndcpDataTransmitResponseResult::Accepted
        );
        assert_eq!(response.link_id, 0, "sent on the MCCH, where the radio still is");
        let alloc = response.alloc.clone().expect("PDCH assignment");
        assert_eq!(
            (alloc.alloc_type, alloc.ul_dl_assigned, ts_of(&alloc), alloc.carrier, alloc.usage),
            (ChanAllocType::Replace, UlDlAssignment::Both, vec![4], Some(MAIN_CARRIER), None),
            "{slots:?}"
        );
        {
            let state = air.test.config.state_read();
            let grant = state.pdch_by_issi[&ISSI];
            assert_eq!(
                grant.slot,
                CarrierSlot {
                    carrier_num: MAIN_CARRIER,
                    ts: 4
                }
            );
            assert_eq!(state.timeslot_alloc.slot_owner(grant.slot), Some(TimeslotOwner::PacketData));
        }
        air.run(2);
        assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
        air.take_sn();
        air.send_unack_on(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x51, "/status.wml"))), 4);
        let answer = air.next_sn(300).expect("the answer");
        assert_eq!((answer.sn_type(), answer.link_id, answer.stealing), (Some(4), 4, false));
        wtp_down(&answer, ip);
    }
}

/// With its preferred slots busy the PDCH takes the next one; with none free the data stays on
/// the MCCH (RESPONSE accepted, no assignment).
#[test]
fn pdch_takes_the_next_preferred_slot_or_the_data_stays_on_the_mcch() {
    let mut air = Air::new(pdch_config(false));
    air.test
        .config
        .state_write()
        .timeslot_alloc
        .reserve(TimeslotOwner::Cmce, 4)
        .unwrap();
    for (issi, expected) in [(ISSI, Some(3)), (ISSI2, Some(2)), (3_000_001, None)] {
        air.send(issi, &demand(1, None, false));
        air.next_sn(8).unwrap();
        air.send(issi, &transmit_request(1, Some((1, false))));
        let response = air.next_sn(8).unwrap();
        assert_eq!(
            decode_data_transmit_response(&response.sn_buf()).unwrap().result,
            SndcpDataTransmitResponseResult::Accepted
        );
        assert_eq!(response.alloc.as_ref().map(|a| ts_of(a)[0]), expected, "ISSI {issi}");
        assert_eq!(pdch_slot_of(&air, issi), expected);
    }
}

/// SN-END OF DATA from the radio on its PDCH: answered there with "quit and go" back to the
/// MCCH, and the slot is free once that went out.
#[test]
fn end_of_data_sends_the_radio_back_and_frees_the_slot() {
    debug::setup_logging_verbose();
    let mut air = Air::new(pdch_config(false));
    onto_pdch(&mut air, ISSI);
    air.send_on(ISSI, &end_of_data(), 4);
    let eod = air.next_sn(8).expect("END OF DATA");
    assert_eq!((eod.sn_type(), eod.link_id), (Some(8), 4));
    let alloc = eod.alloc.clone().expect("back to the MCCH");
    assert_eq!((alloc.alloc_type, ts_of(&alloc)), (ChanAllocType::QuitAndGo, vec![]));
    air.run(2);
    let state = air.test.config.state_read();
    assert!(state.pdch_by_issi.is_empty());
    assert_eq!(state.timeslot_alloc.owner(4), None, "slot free");
}

/// SN-END OF DATA with "immediate service change": the radio has left its PDCH already (clause
/// 28.2.4.7 NOTE 3). No answer, and the slot is free at once.
#[test]
fn end_of_data_with_immediate_service_change_gets_no_answer() {
    let mut air = Air::new(pdch_config(false));
    onto_pdch(&mut air, ISSI);
    let eod = encode_end_of_data(&SndcpEndOfData {
        immediate_service_change: true,
    })
    .unwrap()
    .to_bitstr();
    air.send_on(ISSI, &eod, 4);
    air.run(40);
    assert!(air.take_sn().is_empty(), "no SN-END OF DATA back");
    let state = air.test.config.state_read();
    assert!(state.pdch_by_issi.is_empty());
    assert_eq!(state.timeslot_alloc.owner(4), None, "slot free");
}

/// READY expiring on the PDCH ends it the same way.
#[test]
fn ready_expiry_on_the_pdch_sends_the_radio_back() {
    let mut cfg = pdch_config(false);
    cfg.packet_data.ready_timer_code = 8; // 8 s here, before the 10 s idle release
    let mut air = Air::new(cfg);
    onto_pdch(&mut air, ISSI);
    air.run(540);
    assert!(air.take_sn().is_empty());
    let eod = air.next_sn(120).expect("END OF DATA at READY expiry");
    assert_eq!((eod.sn_type(), eod.link_id), (Some(8), 4));
    assert_eq!(eod.alloc.map(|a| a.alloc_type), Some(ChanAllocType::QuitAndGo));
    air.run(2);
    assert!(air.test.config.state_read().pdch_by_issi.is_empty());
}

/// A slot a call or another radio's PDCH has just freed waits a multiframe before it becomes a
/// PDCH, so a radio still on it sees the AACH unallocated and leaves; the next preferred slot is
/// taken meanwhile.
#[test]
fn a_slot_just_freed_waits_before_it_becomes_a_pdch() {
    let mut air = Air::new(pdch_config(false));
    air.test
        .config
        .state_write()
        .timeslot_alloc
        .reserve(TimeslotOwner::Cmce, 4)
        .unwrap();
    air.run(4);
    air.test
        .config
        .state_write()
        .timeslot_alloc
        .release(TimeslotOwner::Cmce, 4)
        .unwrap();
    onto_pdch(&mut air, ISSI);
    assert_eq!(pdch_slot_of(&air, ISSI), Some(3), "ts4 was a call's until now");
    air.run(80);
    onto_pdch(&mut air, ISSI2);
    assert_eq!(pdch_slot_of(&air, ISSI2), Some(4), "ts4 has waited long enough");
    air.send_on(ISSI, &end_of_data(), 3);
    air.next_sn(8).expect("END OF DATA");
    air.run(2);
    assert_eq!(pdch_slot_of(&air, ISSI), None);
    onto_pdch(&mut air, 3_000_001);
    assert_eq!(pdch_slot_of(&air, 3_000_001), Some(2), "ts3 was a PDCH until now");
}

/// Without data for `pdch_idle_release_secs` a radio in READY is sent back to the MCCH with
/// SN-END OF DATA (quit and go) on its PDCH, which goes once that went out (clause 28.2.6.2
/// NOTE 1); a radio with data going on keeps its PDCH.
#[test]
fn an_idle_pdch_ends_with_end_of_data() {
    let mut cfg = pdch_config(true);
    cfg.packet_data.ready_timer_code = 11; // 60 s: READY stays out of the way
    cfg.packet_data.pdch_idle_release_secs = 2;
    let mut air = Air::new(cfg);
    onto_pdch(&mut air, ISSI);
    let ip2 = onto_pdch(&mut air, ISSI2);
    assert_eq!((pdch_slot_of(&air, ISSI), pdch_slot_of(&air, ISSI2)), (Some(4), Some(3)));
    for i in 0..5u16 {
        air.send_unack_on(
            ISSI2,
            &unitdata(1, &datagram(ip2, GATEWAY, 9201, &wtp_get(0x60 + i, "/status.wml"))),
            3,
        );
        air.run(60);
    }
    let sent: Vec<Down> = air.take_sn().into_iter().filter(|d| d.issi == ISSI).collect();
    assert_eq!(sent.len(), 1, "one SN-END OF DATA to the idle radio: {sent:?}");
    assert_eq!((sent[0].sn_type(), sent[0].link_id), (Some(8), 4), "on its PDCH");
    assert_eq!(sent[0].alloc.as_ref().map(|a| a.alloc_type), Some(ChanAllocType::QuitAndGo));
    assert_eq!(pdch_slot_of(&air, ISSI), None, "idle PDCH released");
    assert_eq!(air.test.config.state_read().timeslot_alloc.owner(4), None);
    assert_eq!(pdch_slot_of(&air, ISSI2), Some(3), "the busy one is kept");
}

/// Back on the MCCH (anything from it on ts1), the radio gets its PDCH assigned again with the
/// next TRANSMIT REQUEST; on its PDCH, a TRANSMIT REQUEST is answered there without one.
#[test]
fn a_radio_back_on_the_mcch_is_assigned_its_pdch_again() {
    let mut air = Air::new(pdch_config(false));
    onto_pdch(&mut air, ISSI);
    air.send(ISSI, &transmit_request(1, None));
    let again = air.next_sn(8).unwrap();
    assert_eq!(again.link_id, 0, "on the MCCH");
    assert_eq!(again.alloc.as_ref().map(ts_of), Some(vec![4]), "assigned again");
    air.run(2);
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
    air.send_on(ISSI, &transmit_request(1, None), 4);
    let there = air.next_sn(8).unwrap();
    assert_eq!((there.link_id, there.chan_alloc), (4, false), "on its PDCH, no new assignment");
    // SN-RECONNECT from the MCCH (after a call, say) is answered like a TRANSMIT REQUEST.
    let reconnect = encode_reconnect(&SndcpReconnect {
        nsapi: Some(1),
        resource_request: SndcpPacketDataResourceRequest::None,
    })
    .unwrap()
    .to_bitstr();
    air.send(ISSI, &reconnect);
    let response = air.next_sn(8).unwrap();
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    assert_eq!((response.link_id, response.alloc.as_ref().map(ts_of)), (0, Some(vec![4])));
}

/// SN-RECONNECT without data to send from the MCCH, as a radio sends it after losing its PDCH or
/// coming back from a call (clause 28.3.4.2 c, table 28.18): no answer, no PDCH, its old one given
/// back. The context stays (STANDBY): its next request gets its answer.
#[test]
fn a_reconnect_without_data_gets_no_answer_and_no_pdch() {
    let mut air = Air::new(pdch_config(false));
    onto_pdch(&mut air, ISSI);
    let reconnect = encode_reconnect(&SndcpReconnect {
        nsapi: None,
        resource_request: SndcpPacketDataResourceRequest::None,
    })
    .unwrap()
    .to_bitstr();
    air.send(ISSI, &reconnect);
    air.run(80);
    assert!(air.take_sn().is_empty(), "no SN-PDU back");
    assert_eq!(pdch_slot_of(&air, ISSI), None, "no PDCH");
    assert_eq!(air.test.config.state_read().timeslot_alloc.owner(4), None);
    air.send(ISSI, &transmit_request(1, None));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    assert!(response.alloc.is_some(), "a PDCH again");
}

/// The radio never acknowledges the RESPONSE carrying its assignment (it did not get it): once
/// the LLC gives up, the PDCH goes.
#[test]
fn an_assignment_never_acknowledged_gives_the_pdch_back() {
    let mut air = Air::new(pdch_config(false));
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.run(4);
    air.send(ISSI, &transmit_request(1, Some((1, false))));
    air.acks.retain(|a| a.0 != ISSI);
    let mut responses = 0;
    for _ in 0..200 {
        air.step();
        air.acks.retain(|a| a.0 != ISSI);
        responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
    }
    assert!(responses >= 2, "the RESPONSE went again");
    assert_eq!(pdch_slot_of(&air, ISSI), None, "PDCH given back");
    assert_eq!(air.test.config.state_read().timeslot_alloc.owner(4), None);
}

/// A radio on its PDCH that sends a PDU on the MCCH (a call set-up, say) is there: its PDUs go on
/// the MCCH, not on the PDCH it left.
#[test]
fn a_radio_heard_on_the_mcch_gets_its_pdus_there() {
    let mut air = Air::new(pdch_config(false));
    onto_pdch(&mut air, ISSI);
    let mut udata = BitBuffer::new_autoexpand(32);
    BlUdata { has_fcs: false }.to_bitbuf(&mut udata);
    append_bits(&mut udata, "0010101010"); // MM
    air.uplink(ISSI, udata);
    assert!(!air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.run(3);
    let d = air.down.iter().find(|d| d.issi == ISSI).expect("a PDU went down");
    assert_eq!((d.link_id, d.stealing), (0, false), "on the MCCH");
}

/// Voice takes the PDCH slot: the grant goes within a second and the radio's data goes on on
/// the MCCH.
#[test]
fn voice_takes_the_pdch_and_the_data_goes_on_on_the_mcch() {
    debug::setup_logging_verbose();
    let mut air = Air::new(pdch_config(true));
    let ip = onto_pdch(&mut air, ISSI);
    voice_takes_the_pdch(&air.test.config);
    air.run(80);
    assert!(air.test.config.state_read().pdch_by_issi.is_empty());
    air.send_unack(ISSI, &unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x70, "/status.wml"))));
    let answer = air.next_sn(300).expect("answer");
    assert_eq!((answer.sn_type(), answer.link_id), (Some(4), 0));
}

/// Voice first: a radio with a call up in one of its groups gets no PDCH (it would be taken back
/// within a second); the RESPONSE assigns none. READY ending during the call sends no SN-END OF
/// DATA (the radio listens to the traffic channel). After the call it gets its PDCH.
#[test]
fn a_radio_in_a_call_gets_no_pdch_and_no_end_of_data() {
    let mut cfg = pdch_config(false);
    cfg.packet_data.ready_timer_code = 8; // 8 s here
    let mut air = Air::new(cfg);
    {
        let mut state = air.test.config.state_write();
        state.subscribers.register(ISSI);
        state.subscribers.affiliate(ISSI, GSSI);
        state.active_call_ts.insert(GSSI, (MAIN_CARRIER, 2, 4));
    }
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.send(ISSI, &transmit_request(1, Some((1, false))));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    assert!(response.alloc.is_none(), "no PDCH during the call");
    assert_eq!(pdch_slot_of(&air, ISSI), None);
    air.run(700);
    assert!(air.take_sn().is_empty(), "no SN-END OF DATA to a radio in a call");
    air.test.config.state_write().active_call_ts.clear();
    air.send(ISSI, &transmit_request(1, None));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(response.alloc.as_ref().map(ts_of), Some(vec![4]), "a PDCH after the call");
}

// With call control ---------------------------------------------------------------------------

const DATA_RADIO: u32 = 4_000_001;

/// The data radio's PDP context and PDCH through the whole stack.
fn voice_onto_pdch(v: &mut Voice) {
    v.uplink(
        DATA_RADIO,
        MleProtocolDiscriminator::Sndcp,
        BitBuffer::from_bitstr(&demand(1, None, false)),
    );
    v.run(8);
    v.uplink(
        DATA_RADIO,
        MleProtocolDiscriminator::Sndcp,
        BitBuffer::from_bitstr(&transmit_request(1, Some((1, false)))),
    );
    v.run(8);
    let grant = v.test.config.state_read().pdch_by_issi.get(&DATA_RADIO).copied();
    assert!(grant.is_some_and(|g| g.on_air && g.slot.ts == 4), "{grant:?}");
}

/// Group calls from three radios to three groups; returns the circuits opened (carrier, ts).
fn three_group_calls(v: &mut Voice, emergency_last: bool) -> Vec<(u16, u8)> {
    use tetra_saps::control::call_control::CallControl;
    for (i, g) in [101u32, 102, 103].into_iter().enumerate() {
        v.register(2_000_001 + i as u32, Some(g));
    }
    for (i, g) in [101u32, 102, 103].into_iter().enumerate() {
        let priority = if emergency_last && i == 2 { 15 } else { 0 };
        v.cmce(3_000_001 + i as u32, u_setup(g, true, false, priority));
        v.run(4);
    }
    v.run(80);
    v.msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(CallControl::Open(c)) => Some((c.carrier_num, c.ts)),
            _ => None,
        })
        .collect()
}

/// A group call in a full cell takes the PDCH slot: no call is cut, the radio loses its PDCH
/// (its data goes on on the MCCH), and the group's D-SETUP goes on the MCCH as always.
#[test]
fn a_group_call_in_a_full_cell_takes_the_pdch() {
    debug::setup_logging_verbose();
    let mut v = Voice::new(voice_config(Some(PacketDataBearer::Pdch), false), false);
    voice_onto_pdch(&mut v);
    let opens = three_group_calls(&mut v, false);
    assert_eq!(opens, vec![(MAIN_CARRIER, 2), (MAIN_CARRIER, 3), (MAIN_CARRIER, 4)]);
    assert_eq!(count(&v.log, "CallEnded"), 0, "no call cut");
    let state = v.test.config.state_read();
    assert!(state.pdch_by_issi.is_empty(), "the PDCH went to the call");
    assert_eq!(state.timeslot_alloc.owner(4), Some(TimeslotOwner::Cmce));
    drop(state);
    let d_setup_103 = v
        .msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::TmaUnitdataReq(req) if req.main_address.ssi == 103 => Some(req.link_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !d_setup_103.is_empty() && d_setup_103.iter().all(|l| *l == 0),
        "group signalling on the MCCH"
    );
}

/// A member of a group on its PDCH listens there, not to the MCCH: the group's D-SETUP goes on the
/// MCCH and on that PDCH, so it joins the call at once; the PDCH then goes within a second.
#[test]
fn a_group_call_set_up_reaches_a_member_on_its_pdch() {
    debug::setup_logging_verbose();
    let mut v = Voice::new(voice_config(Some(PacketDataBearer::Pdch), false), false);
    v.register(DATA_RADIO, Some(GSSI));
    voice_onto_pdch(&mut v);
    v.register(ISSI, Some(GSSI));
    v.msgs.clear();
    v.cmce(ISSI, u_setup(GSSI, true, false, 0));
    v.run(10);
    let links: Vec<u32> = v
        .msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::TmaUnitdataReq(req) if req.main_address.ssi == GSSI => Some(req.link_id),
            _ => None,
        })
        .collect();
    assert!(
        links.contains(&0) && links.contains(&4),
        "the group's D-SETUP on the MCCH and on the member's PDCH: {links:?}"
    );
    v.run(80);
    assert!(v.test.config.state_read().pdch_by_issi.is_empty(), "the PDCH goes for the call");
}

/// With two carriers voice fills the secondary before touching the PDCH.
#[test]
fn with_two_carriers_voice_uses_the_secondary_before_the_pdch() {
    let mut v = Voice::new(voice_config(Some(PacketDataBearer::Pdch), true), false);
    voice_onto_pdch(&mut v);
    let opens = three_group_calls(&mut v, false);
    assert_eq!(opens, vec![(MAIN_CARRIER, 2), (MAIN_CARRIER, 3), (SECONDARY_CARRIER, 1)]);
    let grant = v.test.config.state_read().pdch_by_issi.get(&DATA_RADIO).copied();
    assert!(grant.is_some_and(|g| g.on_air && g.slot.ts == 4), "PDCH untouched");
}

/// An emergency call in a cell whose last slot is the PDCH takes the PDCH, not a call.
#[test]
fn an_emergency_call_takes_the_pdch_before_cutting_a_call() {
    let mut v = Voice::new(voice_config(Some(PacketDataBearer::Pdch), false), false);
    voice_onto_pdch(&mut v);
    let opens = three_group_calls(&mut v, true);
    assert_eq!(opens.last(), Some(&(MAIN_CARRIER, 4)));
    assert_eq!(count(&v.log, "CallEnded"), 0, "no call pre-empted");
    assert!(v.test.config.state_read().pdch_by_issi.is_empty());
}

// The dashboard -------------------------------------------------------------------------------

/// An LLC PDU from `DATA_RADIO` on its PDCH (ts4) through the real MAC, as a MAC-U-BLCK.
fn uplink_on_the_pdch(v: &mut Voice, llc: BitBuffer) {
    // Received in the tick whose downlink slot is ts2: sent on ts4 two slots before.
    while v.ticks % 4 != 1 {
        v.run(1);
    }
    v.test.submit_message(mac_u_blck_with(5, 15, &llc));
    v.run(1);
}

/// SN-PDU `sn` from `DATA_RADIO` in a BL-DATA (acknowledged) or BL-UDATA.
fn sn_in_bl(v: &mut Voice, sn: &str, ack: bool) -> BitBuffer {
    let mut pdu = BitBuffer::new_autoexpand(64);
    if ack {
        let ns = v.ns.entry(DATA_RADIO).or_insert(0);
        BlData { has_fcs: false, ns: *ns }.to_bitbuf(&mut pdu);
        *ns ^= 1;
    } else {
        BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    }
    append_bits(&mut pdu, &format!("100{sn}"));
    pdu
}

fn pdch_telemetry(source: &tetra_entities::net_telemetry::TelemetrySource) -> Vec<tetra_entities::net_telemetry::TelemetryEvent> {
    use tetra_entities::net_telemetry::TelemetryEvent;
    std::iter::from_fn(|| source.try_recv())
        .filter(|e| matches!(e, TelemetryEvent::PdchChanged { .. } | TelemetryEvent::TsDataActivity { .. }))
        .collect()
}

/// The PDCH on the dashboard: the MAC reports the slot becoming the radio's PDCH, every PDU of the
/// radio there (both ways) and the slot being released; the dashboard turns that into `pdch`
/// active/inactive, `ts_data` at most every 250 ms per slot, and the `pdch` list of its snapshot.
#[test]
fn the_dashboard_shows_the_pdch_and_its_traffic() {
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};
    debug::setup_logging_verbose();
    let (sink, source) = telemetry_channel();
    let mut v = Voice::with_telemetry(voice_config(Some(PacketDataBearer::Pdch), false), true, Some(sink));

    voice_onto_pdch(&mut v);
    let onto = pdch_telemetry(&source);
    let active = TelemetryEvent::PdchChanged {
        carrier_num: MAIN_CARRIER,
        ts: 4,
        issi: DATA_RADIO,
        active: true,
    };
    assert_eq!(
        format!("{onto:?}"),
        format!("{:?}", [active.clone()]),
        "only the PDCH: signalling on the MCCH is no data"
    );

    // Traffic on the PDCH: a datagram up, three down, then END OF DATA up and its answer down.
    let up = sn_in_bl(&mut v, &unitdata(1, &[0u8; 8]), false);
    uplink_on_the_pdch(&mut v, up);
    for _ in 0..3 {
        v.test.submit_message(tl_unitdata_to(TetraAddress::issi(DATA_RADIO), 16, None));
    }
    v.run(4);
    let traffic = pdch_telemetry(&source);
    let eod = sn_in_bl(&mut v, &end_of_data(), true);
    uplink_on_the_pdch(&mut v, eod);
    v.run(40);
    assert!(v.test.config.state_read().pdch_by_issi.is_empty(), "END OF DATA frees the PDCH");
    let release = pdch_telemetry(&source);

    let data = |events: &[TelemetryEvent], uplink: bool| {
        events
            .iter()
            .filter(|e| {
                matches!(e, TelemetryEvent::TsDataActivity { carrier_num: MAIN_CARRIER, ts: 4, issi: DATA_RADIO, uplink: u } if *u == uplink)
            })
            .count()
    };
    assert_eq!((data(&traffic, true), data(&traffic, false)), (1, 3), "{traffic:?}");
    assert_eq!(traffic.len(), 4, "{traffic:?}");
    assert!(data(&release, true) == 1 && data(&release, false) >= 1, "{release:?}");
    let inactive = TelemetryEvent::PdchChanged {
        carrier_num: MAIN_CARRIER,
        ts: 4,
        issi: DATA_RADIO,
        active: false,
    };
    assert_eq!(format!("{:?}", release.last()), format!("{:?}", Some(&inactive)), "{release:?}");
    assert_eq!(
        release.len(),
        data(&release, true) + data(&release, false) + 1,
        "nothing after the release: {release:?}"
    );

    // The dashboard.
    let dir = std::env::temp_dir().join(format!("pdch_dashboard_{}", std::process::id()));
    let server = tetra_entities::net_dashboard::DashboardServer::new(dir.join("config.toml").to_string_lossy().into_owned());
    let ws = server.subscribe();
    let json = |s: String| serde_json::from_str::<serde_json::Value>(&s).unwrap();
    let snapshot_pdch = |server: &tetra_entities::net_dashboard::DashboardServer| json(server.snapshot_json().unwrap())["pdch"].clone();
    assert_eq!(snapshot_pdch(&server), serde_json::json!([]));
    for e in onto {
        server.handle_telemetry(e);
    }
    let snapshot = server.snapshot_json().unwrap();
    println!("snapshot pdch: {}", json(snapshot.clone())["pdch"]);
    assert_eq!(
        json(snapshot)["pdch"],
        serde_json::json!([{"carrier_num": MAIN_CARRIER, "ts": 4, "issi": DATA_RADIO, "since_secs": 0}])
    );
    for e in traffic.into_iter().chain(release) {
        server.handle_telemetry(e);
    }
    let sent: Vec<String> = std::iter::from_fn(|| ws.try_recv().ok()).collect();
    for m in &sent {
        println!("ws: {m}");
    }
    assert_eq!(
        sent,
        vec![
            format!(r#"{{"active":true,"carrier_num":{MAIN_CARRIER},"issi":{DATA_RADIO},"ts":4,"type":"pdch"}}"#),
            format!(r#"{{"carrier_num":{MAIN_CARRIER},"dir":"ul","issi":{DATA_RADIO},"ts":4,"type":"ts_data"}}"#),
            format!(r#"{{"active":false,"carrier_num":{MAIN_CARRIER},"issi":{DATA_RADIO},"ts":4,"type":"pdch"}}"#),
        ],
        "one ts_data for the whole burst (4 Hz per slot)"
    );
    assert_eq!(snapshot_pdch(&server), serde_json::json!([]));
}

/// With the bearer on the MCCH there is no PDCH: the MAC reports neither PDCH nor data activity.
#[test]
fn no_pdch_telemetry_with_the_bearer_on_the_mcch() {
    use tetra_entities::net_telemetry::telemetry_channel;
    let (sink, source) = telemetry_channel();
    let mut v = Voice::with_telemetry(voice_config(Some(PacketDataBearer::Mcch), false), true, Some(sink));
    v.uplink(
        DATA_RADIO,
        MleProtocolDiscriminator::Sndcp,
        BitBuffer::from_bitstr(&demand(1, None, false)),
    );
    v.run(8);
    v.uplink(
        DATA_RADIO,
        MleProtocolDiscriminator::Sndcp,
        BitBuffer::from_bitstr(&transmit_request(1, Some((1, false)))),
    );
    v.run(8);
    let up = sn_in_bl(&mut v, &unitdata(1, &[0u8; 8]), false);
    uplink_on_the_pdch(&mut v, up);
    v.test.submit_message(tl_unitdata_to(TetraAddress::issi(DATA_RADIO), 16, None));
    v.run(12);
    assert!(pdch_telemetry(&source).is_empty());
}

// ---------------------------------------------------------------------------------------------
// One slot per radio: what goes on the air does not change with multislot packet data
// ---------------------------------------------------------------------------------------------

/// Fingerprint of every slot `scenario` put on the air and every PDU passed between its MAC and
/// its LLC. The constants below were recorded on miura's single-slot code (79e47cc): a different
/// value is a change on the air with one slot per radio, never a reason to update the constant.
fn fingerprint(scenario: fn()) -> u64 {
    common::component_test::trace_arm();
    scenario();
    let hash = common::component_test::trace_hash();
    println!("fingerprint {hash:#018x}");
    hash
}

#[test]
fn width1_identity_al_on_pdch() {
    assert_eq!(
        fingerprint(the_advanced_link_of_a_radio_on_its_pdch_runs_there),
        0x9155_f171_4d09_5be6
    );
}

#[test]
fn width1_identity_voice_preemption() {
    assert_eq!(
        fingerprint(voice_taking_the_pdch_slot_shows_traffic_and_drops_its_data),
        0x2fa7_851a_5390_c01e
    );
}

#[test]
fn width1_identity_uplink_on_pdch() {
    assert_eq!(fingerprint(the_dashboard_shows_the_pdch_and_its_traffic), 0x7287_e11a_591f_9a21);
}

#[test]
fn width1_identity_group_copy() {
    assert_eq!(fingerprint(a_group_call_set_up_reaches_a_member_on_its_pdch), 0x99a8_d4f3_31c6_d159);
}

#[test]
fn width1_identity_fragmented_datagram() {
    assert_eq!(
        fingerprint(group_call_setup_is_not_held_behind_a_fragmented_datagram),
        0x7fbb_8cd1_36e0_d68f
    );
}

// Two carriers and multislot channels of the main carrier: recorded on miura before the
// packet-data carrier (c4fc51a); a different value is a change on the air, never a reason to
// update the constant.

#[test]
fn dual_identity_voice_before_the_pdch() {
    assert_eq!(
        fingerprint(with_two_carriers_voice_uses_the_secondary_before_the_pdch),
        0x92be_16fd_a974_2435
    );
}

#[test]
fn dual_identity_duplex_calls() {
    assert_eq!(
        fingerprint(duplex_calls_on_two_carriers_are_the_same_with_packet_data_on),
        0xdb61_bf54_cae9_5f0b
    );
}

#[test]
fn multislot_identity_al_spread() {
    assert_eq!(fingerprint(al_segments_spread_over_the_multislot_pdch), 0x26d6_f139_7a3f_1333);
}

#[test]
fn multislot_identity_uplink_across_the_channel() {
    assert_eq!(fingerprint(uplink_grant_and_reassembly_across_the_channel), 0xfa06_aeaf_36d6_3e98);
}

#[test]
fn multislot_identity_voice_releases_the_channel() {
    assert_eq!(
        fingerprint(voice_taking_one_slot_releases_the_whole_multislot_pdch),
        0xb5f2_aab5_94b2_7a8c
    );
}

#[test]
fn multislot_identity_two_radios() {
    assert_eq!(fingerprint(two_radios_never_share_a_slot), 0x4b73_4e56_3d67_6772);
}

// ---------------------------------------------------------------------------------------------
// Multislot PDCH (`pdch_max_slots` > 1): a packet-data channel of several main-carrier slots
// ---------------------------------------------------------------------------------------------

/// `pdch_config` with up to `n` slots per radio.
fn multislot_config(wap: bool, n: u8) -> StackConfig {
    let mut cfg = pdch_config(wap);
    cfg.packet_data.pdch_max_slots = n;
    cfg
}

/// Give `issi` the packet-data channel of `slots` (the first one its grant's) in the shared
/// state, as the SNDCP runtime does.
fn grant_pdch_slots(config: &SharedConfig, issi: u32, slots: &[u8], on_air: bool) -> Vec<CarrierSlot> {
    let mut state = config.state_write();
    let reserved = state.timeslot_alloc.reserve_packet_data_slots(slots, slots.len());
    assert_eq!(reserved.len(), slots.len(), "slots free");
    state.pdch_by_issi.insert(issi, PdchGrant { slot: reserved[0], on_air });
    if reserved.len() > 1 {
        let mut bits = [false; 4];
        for s in &reserved {
            bits[usize::from(s.ts) - 1] = true;
        }
        state.pdch_timeslots_by_issi.insert(issi, bits);
    }
    reserved
}

/// The MS's reading of a slot grant received in downlink slot `grant_dl` on a channel of the
/// slots `channel` (EN 300 392-2 23.5.2.2.2, 23.5.2.2.4): the same-numbered uplink slot is
/// opportunity 0; `delay` more slots of the channel, frame 18 included; then `slots` successive
/// slots of the channel, jumping the common linearization ones of frame 18.
fn ms_granted_labels(grant_dl: TdmaTime, delay: usize, slots: usize, channel: &[u8]) -> Vec<TdmaTime> {
    let next = |mut t: TdmaTime| loop {
        t = t.add_timeslots(1);
        if channel.contains(&t.t) {
            return t;
        }
    };
    let mut t = grant_dl;
    for _ in 0..delay {
        t = next(t);
    }
    let mut out = Vec::new();
    while out.len() < slots {
        if !t.is_mandatory_clch() {
            out.push(t);
        }
        t = next(t);
    }
    out
}

/// The MAC-RESOURCEs of the SCH/F block of a downlink slot.
fn mac_resources(slot: &tetra_saps::tmv::TmvUnitdataReqSlot) -> Vec<tetra_pdus::umac::pdus::mac_resource::MacResource> {
    use tetra_pdus::umac::pdus::mac_resource::MacResource;
    let Some(blk) = slot
        .blk1
        .as_ref()
        .filter(|b| b.logical_channel == tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF)
    else {
        return Vec::new();
    };
    let mut block = blk.mac_block.clone();
    block.seek(0);
    let bits = block.to_bitstr();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 16 <= bits.len() {
        let mut b = BitBuffer::from_bitstr(&bits[pos..]);
        b.seek(0);
        if b.peek_bits(2) != Some(0) {
            break;
        }
        let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
        if res.addr.is_none() {
            break;
        }
        let li = res.length_ind;
        out.push(res);
        if li == 0b111111 || li == 0 {
            break;
        }
        pos += usize::from(li) * 8;
    }
    out
}

/// The slot grant a downlink slot carries for `ssi`: (slots, delay), a subslot counting as a slot.
fn grant_for(slot: &tetra_saps::tmv::TmvUnitdataReqSlot, ssi: u32) -> Option<(usize, usize)> {
    use tetra_pdus::umac::enums::basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc;
    use tetra_pdus::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;
    let g = mac_resources(slot)
        .into_iter()
        .find(|r| r.addr.is_some_and(|a| a.ssi == ssi))?
        .slot_granting_element?;
    let delay = match g.granting_delay {
        BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity => 0,
        BasicSlotgrantGrantingDelay::DelayNOpportunities(n) => usize::from(n),
        other => panic!("unexpected granting delay {other:?}"),
    };
    let slots = match g.capacity_allocation {
        BasicSlotgrantCapAlloc::FirstSubslotGranted | BasicSlotgrantCapAlloc::SecondSubslotGranted => 1,
        other => other.to_req_slotcount(),
    };
    Some((slots, delay))
}

/// The UMAC alone, the LLC and the LMAC as sinks, ticking from 0/1/1/1 (tick k has downlink
/// time 0/1/1/1 + k and receives the uplink of two slots earlier).
struct UmacAir {
    test: ComponentTest,
    ticks: i32,
    slots: Vec<tetra_saps::tmv::TmvUnitdataReqSlot>,
    up: Vec<TmaUnitdataInd>,
}

impl UmacAir {
    fn new(cfg: StackConfig) -> Self {
        let mut test = ComponentTest::from_config(cfg, Some(TdmaTime::default()));
        test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc, TetraEntity::Lmac]);
        Self {
            test,
            ticks: 0,
            slots: Vec::new(),
            up: Vec::new(),
        }
    }

    /// Downlink time of the next tick.
    fn next_time(&self) -> TdmaTime {
        TdmaTime::default().add_timeslots(self.ticks)
    }

    fn tick(&mut self) {
        self.test.run_stack(Some(1));
        self.ticks += 1;
        for m in self.test.dump_sinks() {
            match m.msg {
                SapMsgInner::TmaUnitdataInd(ind) => self.up.push(ind),
                SapMsgInner::TmvUnitdataReqSlots(slots) => self.slots.extend(slots.slots),
                _ => {}
            }
        }
    }

    /// `msg` as received in uplink slot `label` (in the tick whose downlink time is two later).
    fn uplink_at(&mut self, label: TdmaTime, msg: SapMsg) {
        let at = label.add_timeslots(2);
        while self.next_time() != at {
            assert!(self.next_time().diff(at) < 0, "uplink slot {label} is past");
            self.tick();
        }
        self.test.submit_message(msg);
        self.tick();
    }

    /// Run until a downlink slot carries a slot grant for `ssi`: that slot's time and the grant
    /// (slots, delay).
    fn next_grant(&mut self, ssi: u32, ticks: usize) -> (TdmaTime, usize, usize) {
        let from = self.slots.len();
        for _ in 0..ticks {
            self.tick();
            if let Some((t, g)) = self.slots[from..].iter().find_map(|s| grant_for(s, ssi).map(|g| (s.ts, g))) {
                return (t, g.0, g.1);
            }
        }
        panic!("no grant for {ssi}");
    }
}

/// `pdu` as the LMAC hands a full uplink slot (SCH/F, CRC passed) of the main carrier to the UMAC.
fn from_lmac(pdu: BitBuffer) -> SapMsg {
    SapMsg {
        sap: Sap::TmvSap,
        src: TetraEntity::Lmac,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmvUnitdataInd(tetra_saps::tmv::TmvUnitdataInd {
            carrier_num: MAIN_CARRIER,
            pdu,
            block_num: tetra_core::PhyBlockNum::Both,
            logical_channel: tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF,
            crc_pass: true,
            scrambling_code: 0,
            rssi_dbfs: f32::NEG_INFINITY,
        }),
    }
}

/// An uplink slot as the radio sends it: `header`, then the TM-SDU part `sdu`, then fill bits (a
/// one, then zeros) to the end of the slot.
fn uplink_block(header: &str, sdu: &str) -> BitBuffer {
    let mut bits = format!("{header}{sdu}1");
    while bits.len() < 268 {
        bits.push('0');
    }
    let mut b = BitBuffer::from_bitstr(&bits);
    b.seek(0);
    b
}

/// A TM-SDU of 400 bits sent up on the channel {3, 4} of the radio: MAC-DATA opening a
/// fragmentation and asking for 2 slots in ts3, then MAC-FRAG and MAC-END in the two slots the
/// grant it gets gives (read from the downlink as the MS does). Returns what reached the LLC, the
/// slots used and the TM-SDU.
fn uplink_across_the_channel(cfg: StackConfig) -> (Vec<TmaUnitdataInd>, Vec<TdmaTime>, String) {
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    let mut air = UmacAir::new(cfg);
    grant_pdch_slots(&air.test.config, ISSI, &[4, 3], true);
    let sdu: String = (0..400).map(|i| if (i * 7) % 3 == 0 { '1' } else { '0' }).collect();
    let (first, frag, end) = (&sdu[..150], &sdu[150..300], &sdu[300..]);
    // MAC-DATA: type 00, fill, not encrypted, address type 00 and address, capacity request with
    // the fragmentation flag and 2 slots, reserved bit.
    let start = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let header = format!("001000{ISSI:024b}11{:04b}0", ReservationRequirement::Req2Slots as u64);
    air.uplink_at(start, from_lmac(uplink_block(&header, first)));
    let (at, slots, delay) = air.next_grant(ISSI, 16);
    assert_eq!(slots, 2, "the 2 slots asked for");
    let labels = ms_granted_labels(at, delay, slots, &[3, 4]);
    // MAC-FRAG: type 01, subtype 0, fill; MAC-END: type 01, subtype 1, fill, length in octets.
    air.uplink_at(labels[0], from_lmac(uplink_block("0101", frag)));
    let end_octets = (10 + end.len() + 1).div_ceil(8);
    let end_header = format!("0111{end_octets:06b}");
    air.uplink_at(labels[1], from_lmac(uplink_block(&end_header, end)));
    for _ in 0..4 {
        air.tick();
    }
    (air.up, labels, sdu)
}

/// An uplink fragmentation continues on any slot of a channel of several slots (EN 300 392-2
/// 23.3.5): the MAC-FRAG and MAC-END in the slots the grant gives, on ts3 and ts4, are
/// reassembled with the MAC-DATA that opened it, and the whole TM-SDU goes up once, with the
/// slot it ended on.
#[test]
fn uplink_grant_and_reassembly_across_the_channel() {
    debug::setup_logging_verbose();
    let (up, labels, sdu) = uplink_across_the_channel(multislot_config(false, 2));
    assert_eq!(labels.iter().map(|l| l.t).collect::<Vec<_>>(), vec![3, 4], "{labels:?}");
    assert_eq!(up.len(), 1, "{up:?}");
    assert_eq!(
        (up[0].main_address.ssi, up[0].link_id, up[0].pdu.as_ref().unwrap().to_bitstr()),
        (ISSI, 4, sdu)
    );
}

/// With PDU association the reservation requirement is in the last PDU of the block (EN 300
/// 392-2 23.5.2.1), and "nothing more to send" is judged per block (23.5.2.3.1): a MAC-DATA
/// with a length and no requirement, followed in its block by one opening a fragmentation and
/// asking for 2 slots, leaves the radio the 3 slots it still holds on its channel {3, 4}. Its
/// MAC-FRAG and MAC-END in the next two are reassembled.
#[test]
fn associated_pdus_keep_the_multislot_reservations_of_their_block() {
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    debug::setup_logging_verbose();
    let mut air = UmacAir::new(multislot_config(false, 2));
    grant_pdch_slots(&air.test.config, ISSI, &[4, 3], true);
    let bits = |n: usize, k: usize| -> String { (0..n).map(|i| if (i * k) % 5 < 2 { '1' } else { '0' }).collect() };
    // MAC-DATA: type 00, fill, not encrypted, address type 00 and address, capacity request
    // (fragmentation flag, requirement, reserved bit) or length indication.
    let with_req = |fill: bool, frag: bool, req: ReservationRequirement| {
        format!("00{}000{ISSI:024b}1{}{:04b}0", u8::from(fill), u8::from(frag), req as u64)
    };
    let s0 = bits(100, 3);
    let start = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let header = with_req(true, false, ReservationRequirement::Req4Slots);
    air.uplink_at(start, from_lmac(uplink_block(&header, &s0)));
    let (at, slots, delay) = air.next_grant(ISSI, 16);
    assert_eq!(slots, 4, "the 4 slots asked for");
    let labels = ms_granted_labels(at, delay, slots, &[3, 4]);

    // 37-bit header and 19 bits: 7 octets, no fill bits; then the fragmentation start.
    let s1 = bits(19, 7);
    let s2 = bits(450, 11);
    let (first, frag, end) = (&s2[..150], &s2[150..300], &s2[300..]);
    let header = format!(
        "000000{ISSI:024b}0{:06b}{s1}{}",
        7,
        with_req(true, true, ReservationRequirement::Req2Slots)
    );
    air.uplink_at(labels[0], from_lmac(uplink_block(&header, first)));
    air.uplink_at(labels[1], from_lmac(uplink_block("0101", frag)));
    let end_octets = (10 + end.len() + 1).div_ceil(8);
    air.uplink_at(labels[2], from_lmac(uplink_block(&format!("0111{end_octets:06b}"), end)));
    for _ in 0..4 {
        air.tick();
    }
    let up: Vec<String> = air.up.iter().map(|u| u.pdu.as_ref().unwrap().to_bitstr()).collect();
    assert_eq!(up, vec![s0, s1, s2]);
}

/// A BL-DATA from the radio on a slot of its channel other than the one its PDUs are sent to
/// is acknowledged on the channel, not stolen as if the slot carried a call; and a BL-DATA going
/// down to it meanwhile carries that acknowledgement (BL-ADATA, 22.3.2.3 d).
#[test]
fn a_bl_ack_for_an_uplink_on_another_slot_stays_on_the_pdch() {
    debug::setup_logging_verbose();
    let mut air = Air::new(multislot_config(false, 2));
    grant_pdch_slots(&air.test.config, ISSI, &[4, 3], true);
    let bl_data = |ns: u8| {
        let mut data = BitBuffer::new_autoexpand(32);
        BlData { has_fcs: false, ns }.to_bitbuf(&mut data);
        append_bits(&mut data, "0101010101");
        data
    };
    air.uplink_on(ISSI, bl_data(0), 3);
    air.run(2);
    let ack = air
        .down
        .iter()
        .find(|d| d.issi == ISSI && d.llc == LlcPduType::BlAck)
        .expect("BL-ACK");
    assert!((3..=4).contains(&ack.link_id), "on the channel: {ack:?}");
    assert!(!ack.stealing && !ack.chan_alloc, "{ack:?}");

    // The next uplink on ts3, with a TL-DATA for the radio in the same tick: one BL-ADATA.
    air.down.clear();
    while air.ticks % 4 != 0 {
        air.step();
    }
    air.test.submit_message(tma_ind(ISSI, bl_data(1), 3));
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.step();
    air.run(2);
    let mine: Vec<&Down> = air.down.iter().filter(|d| d.issi == ISSI).collect();
    assert!(mine.iter().all(|d| d.llc != LlcPduType::BlAck), "no separate BL-ACK: {mine:?}");
    let adata = mine.iter().find(|d| d.llc == LlcPduType::BlAdata).expect("BL-ADATA");
    assert_eq!((adata.link_id, adata.stealing), (4, false));
}

/// The AL-SETUP answer gives N.264 at most the slots a radio's channel may have here: the lower
/// of the radio's proposal and `pdch_max_slots` (within `pdch_timeslots`); 1 by default and with
/// the bearer on the MCCH.
#[test]
fn al_setup_answers_the_multislot_cap() {
    let slots = |n: u8| {
        let mut setup = radio_al_setup();
        setup.connection_width = true;
        setup.uplink_timeslots = Some(n - 1);
        setup
    };
    let answer = |cfg: StackConfig, n: u8| {
        let mut air = Air::new(cfg);
        let a = air.al_setup_with(ISSI, slots(n));
        (a.setup_report, a.uplink_timeslots.map(|s| s + 1))
    };
    assert_eq!(
        answer(multislot_config(false, 3), 4),
        (AlSetup::SETUP_REPORT_SERVICE_CHANGE, Some(3))
    );
    assert_eq!(answer(multislot_config(false, 3), 2), (AlSetup::SETUP_REPORT_SUCCESS, Some(2)));
    let mut mcch = config(true, false);
    mcch.packet_data.pdch_max_slots = 3;
    assert_eq!(answer(mcch, 4), (AlSetup::SETUP_REPORT_SERVICE_CHANGE, Some(1)));
    assert_eq!(answer(pdch_config(false), 4), (AlSetup::SETUP_REPORT_SERVICE_CHANGE, Some(1)));
}

/// The AL-DATA / AL-FINAL for `ssi` in a downlink slot: the AL header and the slot grant of its
/// MAC-RESOURCE (slots, delay).
fn al_data_in(slot: &tetra_saps::tmv::TmvUnitdataReqSlot, ssi: u32) -> Vec<(AlData, Option<(usize, usize)>)> {
    use tetra_pdus::umac::enums::basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay;
    use tetra_pdus::umac::pdus::mac_resource::MacResource;
    let Some(blk) = slot
        .blk1
        .as_ref()
        .filter(|b| b.logical_channel == tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF)
    else {
        return Vec::new();
    };
    let mut block = blk.mac_block.clone();
    block.seek(0);
    let bits = block.to_bitstr();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 16 <= bits.len() {
        let mut b = BitBuffer::from_bitstr(&bits[pos..]);
        b.seek(0);
        if b.peek_bits(2) != Some(0) {
            break;
        }
        let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
        let Some(addr) = res.addr else { break };
        if addr.ssi == ssi && b.peek_bits(4) == Some(9) {
            let grant = res.slot_granting_element.as_ref().map(|g| {
                let delay = match g.granting_delay {
                    BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity => 0,
                    BasicSlotgrantGrantingDelay::DelayNOpportunities(n) => usize::from(n),
                    other => panic!("unexpected granting delay {other:?}"),
                };
                (g.capacity_allocation.to_req_slotcount(), delay)
            });
            out.push((AlData::from_bitbuf(&mut b).unwrap(), grant));
        }
        if res.length_ind == 0b111111 || res.length_ind == 0 {
            break;
        }
        pos += usize::from(res.length_ind) * 8;
    }
    out
}

/// An AL segment on the air: (tick, slot, AL header, slot grant (slots, delay)).
type AirSegment = (usize, TdmaTime, AlData, Option<(usize, usize)>);

/// AL segments of a transfer of `octets` to the radio on its PDCH `slots`, through the real MAC,
/// in air order, and the tick its TL-SDU was all out.
fn al_transfer_on(cfg: StackConfig, slots: &[u8], octets: usize) -> (Vec<AirSegment>, usize) {
    let mut mac = MacAir::with(cfg);
    grant_pdch_slots(&mac.test.config, ISSI, slots, true);
    let reporter = mac.transfer(octets);
    let mut segments = Vec::new();
    let mut done = None;
    for tick in 0..4 * 40 {
        mac.test.run_stack(Some(1));
        for msg in mac.test.dump_sinks() {
            let slots = match msg.msg {
                SapMsgInner::TmvUnitdataReq(slot) => vec![slot],
                SapMsgInner::TmvUnitdataReqSlots(slots) => slots.slots,
                _ => continue,
            };
            for slot in slots.iter().filter(|s| s.carrier_num == MAIN_CARRIER) {
                for (header, grant) in al_data_in(slot, ISSI) {
                    segments.push((tick, slot.ts, header, grant));
                }
            }
        }
        if done.is_none() && reporter.get_state() == TxState::Transmitted {
            done = Some(tick);
        }
    }
    (segments, done.expect("the TL-SDU went out"))
}

/// The advanced link of a radio on a channel of two slots: its segments go on both slots, in
/// S(S) order, whole, and the TL-SDU is out in well under the time one slot takes.
#[test]
fn al_segments_spread_over_the_multislot_pdch() {
    debug::setup_logging_verbose();
    let (one, one_done) = al_transfer_on(pdch_config(false), &[4], 300);
    let (two, two_done) = al_transfer_on(multislot_config(false, 2), &[4, 3], 300);
    assert!(one.len() >= 13, "{one:?}");
    let first: Vec<u8> = two.iter().take(13).map(|s| s.2.ss).collect();
    assert_eq!(first, (0..13).collect::<Vec<u8>>(), "in S(S) order");
    assert!(two.iter().all(|s| (3..=4).contains(&s.1.t)), "on the channel");
    assert!(two.iter().any(|s| s.1.t == 3) && two.iter().any(|s| s.1.t == 4), "on both slots");
    assert!(
        two_done * 3 <= one_done * 2,
        "two slots {two_done} ticks, one slot {one_done} ticks"
    );
}

/// On a channel of several slots the segment that asks for an acknowledgement leaves with a
/// grant of one full slot for the answer (EN 300 392-2 23.5.2.2.1 b), a few opportunities on;
/// the other segments with none.
#[test]
fn the_al_ack_comes_in_the_reply_slot() {
    debug::setup_logging_verbose();
    let (segments, _) = al_transfer_on(multislot_config(false, 2), &[4, 3], 100);
    let first: Vec<_> = segments
        .iter()
        .take(5)
        .map(|s| (s.2.ss, s.2.acknowledgement_requested, s.3))
        .collect();
    assert_eq!(first.len(), 5, "{segments:?}");
    for (ss, ar, grant) in first {
        match grant {
            Some((slots, delay)) => assert!(ar && slots == 1 && delay >= 2, "segment {ss}: {grant:?}"),
            None => assert!(!ar, "segment {ss} asks for an answer without a slot for it"),
        }
    }
}

/// SN-DATA TRANSMIT REQUEST asking `asks` slots both ways with a full phase modulation capability
/// of `full` (the MXP600 asks the N.264 of its link, full 4).
fn transmit_request_full(nsapi: u8, asks: u8, full: u8) -> String {
    encode_data_transmit_request(&SndcpDataTransmitRequest {
        nsapi,
        logical_link_status: true,
        resource_request: SndcpPacketDataResourceRequest::PhaseModulation(SndcpPhaseModulationResourceRequest {
            uplink_timeslots: asks,
            downlink_timeslots: asks,
            full_phase_modulation_capability_timeslots: full,
            unspecified_phase_modulation_resource: false,
        }),
    })
    .unwrap()
    .to_bitstr()
}

/// PDP context and the TRANSMIT REQUEST `request`: the timeslots of the channel assigned (none:
/// the data stays on the MCCH); the radio is on it once the RESPONSE went out.
fn onto_channel(air: &mut Air, issi: u32, request: &str) -> Vec<u8> {
    air.send(issi, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.send(issi, request);
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    air.run(2);
    air.take_sn();
    response.alloc.as_ref().map(ts_of).unwrap_or_default()
}

/// Main-carrier timeslots the allocator gives the SNDCP bearer.
fn packet_data_slots(air: &Air) -> Vec<u8> {
    let state = air.test.config.state_read();
    (2..=4u8)
        .filter(|ts| state.timeslot_alloc.owner(*ts) == Some(TimeslotOwner::PacketData))
        .collect()
}

/// By default a radio gets one slot, whatever it asks or can take.
#[test]
fn the_default_keeps_one_slot_whatever_the_radio_asks() {
    let mut air = Air::new(pdch_config(false));
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 4, 4)), vec![4]);
    assert!(air.test.config.state_read().pdch_timeslots_by_issi.is_empty());
    assert_eq!(packet_data_slots(&air), vec![4]);
}

/// A radio that can take 4 slots gets the slots of `pdch_timeslots` (3 at most on the main
/// carrier): one channel allocation with all of them, the grant on the first one preferred, the
/// timeslots published next to it. On a single carrier one traffic slot stays for voice.
#[test]
fn a_request_for_four_slots_gets_the_slots_of_pdch_timeslots() {
    debug::setup_logging_verbose();
    let mut cfg = multislot_config(false, 4);
    cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    let mut air = Air::new(cfg);
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    let alloc = response.alloc.clone().expect("a PDCH");
    assert_eq!(
        (ts_of(&alloc), alloc.carrier, alloc.alloc_type, alloc.ul_dl_assigned),
        (vec![2, 3, 4], Some(MAIN_CARRIER), ChanAllocType::Replace, UlDlAssignment::Both)
    );
    {
        let state = air.test.config.state_read();
        assert_eq!(state.pdch_by_issi[&ISSI].slot.ts, 4);
        assert_eq!(state.pdch_timeslots_by_issi[&ISSI], [false, true, true, true]);
    }
    assert_eq!(packet_data_slots(&air), vec![2, 3, 4]);

    let mut air = Air::new(multislot_config(false, 4));
    assert_eq!(
        onto_channel(&mut air, ISSI, &transmit_request_full(1, 4, 4)),
        vec![3, 4],
        "voice headroom"
    );
    assert_eq!(packet_data_slots(&air), vec![3, 4]);
}

/// The width is the smallest of what the radio can take (its full capability, not what it
/// asks), `pdch_max_slots`, the free slots of `pdch_timeslots` and the voice headroom.
#[test]
fn the_width_is_the_smallest_of_capability_config_free_slots_and_headroom() {
    let case = |max: u8, secondary: bool, voice_on: Option<u8>, request: String| {
        let mut cfg = multislot_config(false, max);
        if secondary {
            cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
        }
        let mut air = Air::new(cfg);
        if let Some(ts) = voice_on {
            air.test
                .config
                .state_write()
                .timeslot_alloc
                .reserve(TimeslotOwner::Cmce, ts)
                .unwrap();
        }
        onto_channel(&mut air, ISSI, &request)
    };
    assert_eq!(
        case(2, false, None, transmit_request_full(1, 1, 4)),
        vec![3, 4],
        "the ask is no limit"
    );
    assert_eq!(case(2, false, None, transmit_request_full(1, 1, 1)), vec![4], "a one-slot radio");
    assert_eq!(
        case(3, false, None, transmit_request_full(1, 4, 4)),
        vec![3, 4],
        "ts2 left for voice"
    );
    assert_eq!(case(3, true, None, transmit_request_full(1, 4, 4)), vec![2, 3, 4]);
    assert_eq!(case(3, true, Some(3), transmit_request_full(1, 4, 4)), vec![2, 4], "not contiguous");
    assert_eq!(
        case(3, false, Some(3), transmit_request_full(1, 4, 4)),
        vec![4],
        "ts2 left for voice"
    );
    assert_eq!(case(3, false, None, transmit_request(1, None)), vec![4], "no resource request");
}

/// After its first session the MXP600 asks the N.264 of its link (1 slot while it was 1): with
/// its full capability of 4 it still gets the channel.
#[test]
fn a_radio_with_an_old_one_slot_link_still_gets_the_channel() {
    let mut air = Air::new(multislot_config(false, 2));
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 1, 4)), vec![3, 4]);
}

/// A TRANSMIT REQUEST on any slot of its channel is from a radio on it: answered there without a
/// new assignment.
#[test]
fn a_request_on_another_slot_of_the_channel_is_not_reassigned() {
    let mut air = Air::new(multislot_config(false, 2));
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 1, 4)), vec![3, 4]);
    air.send_on(ISSI, &transmit_request_full(1, 2, 4), 3);
    let there = air.next_sn(8).expect("RESPONSE");
    assert_eq!((there.link_id, there.chan_alloc), (4, false), "on its PDCH, no new assignment");
}

/// SN-END OF DATA on the channel: "quit and go" back to the MCCH, and every slot of the channel is
/// free once it went out.
#[test]
fn end_of_data_frees_every_slot_of_the_channel() {
    let mut air = Air::new(multislot_config(false, 2));
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 2, 4)), vec![3, 4]);
    air.send_on(ISSI, &end_of_data(), 3);
    let eod = air.next_sn(8).expect("END OF DATA");
    assert_eq!(eod.alloc.as_ref().map(|a| a.alloc_type), Some(ChanAllocType::QuitAndGo));
    air.run(2);
    let state = air.test.config.state_read();
    assert!(state.pdch_by_issi.is_empty() && state.pdch_timeslots_by_issi.is_empty());
    assert_eq!((state.timeslot_alloc.owner(3), state.timeslot_alloc.owner(4)), (None, None));
}

/// An idle channel of several slots ends like one of one slot.
#[test]
fn an_idle_multislot_pdch_ends_with_end_of_data() {
    let mut cfg = multislot_config(false, 2);
    cfg.packet_data.ready_timer_code = 11;
    cfg.packet_data.pdch_idle_release_secs = 2;
    let mut air = Air::new(cfg);
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 2, 4)), vec![3, 4]);
    air.run(300);
    let sent = air.take_sn();
    assert_eq!(sent.iter().filter(|d| d.sn_type() == Some(8)).count(), 1, "{sent:?}");
    assert!(packet_data_slots(&air).is_empty());
    assert!(air.test.config.state_read().pdch_timeslots_by_issi.is_empty());
}

/// Voice taking one slot of a channel of several slots takes the whole channel: the grant and
/// the timeslots go, the other slots back to the pool, which gives them to a PDCH again only
/// after a multiframe.
#[test]
fn voice_taking_one_slot_releases_the_whole_multislot_pdch() {
    let mut cfg = multislot_config(false, 3);
    cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    let mut air = Air::new(cfg);
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 4, 4)), vec![2, 3, 4]);
    {
        let mut state = air.test.config.state_write();
        for ts in 1..=4 {
            state
                .timeslot_alloc
                .reserve_slot(
                    TimeslotOwner::Cmce,
                    CarrierSlot {
                        carrier_num: SECONDARY_CARRIER,
                        ts,
                    },
                )
                .unwrap();
        }
        let taken = state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).unwrap();
        assert_eq!(taken.ts, 2, "the call takes ts2 of the channel");
        assert!(
            state.pdch_channel(ISSI).is_none(),
            "the channel is gone at once for the MAC and the LLC"
        );
    }
    // The SNDCP learns of it at its next housekeeping (about once a second).
    for _ in 0..80 {
        if air.test.config.state_read().pdch_by_issi.is_empty() {
            break;
        }
        air.step();
    }
    {
        let state = air.test.config.state_read();
        assert!(state.pdch_by_issi.is_empty() && state.pdch_timeslots_by_issi.is_empty());
        assert_eq!(state.timeslot_alloc.owner(2), Some(TimeslotOwner::Cmce));
        assert_eq!((state.timeslot_alloc.owner(3), state.timeslot_alloc.owner(4)), (None, None));
    }
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    assert!(
        response.alloc.is_none(),
        "ts3 and ts4 were the channel's less than a multiframe ago"
    );
    air.run(80);
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    let slots = response.alloc.as_ref().map(ts_of).expect("a PDCH again");
    assert!(!slots.is_empty() && !slots.contains(&2), "{slots:?}");
}

/// Every slot of a channel of several slots is an assigned control channel of its radio (AACH
/// assigned control / assigned only, SCH/F Null while idle); a slot outside it is not. Voice
/// taking one slot takes them all back at once.
#[test]
fn a_multislot_pdch_is_assigned_control_on_every_slot() {
    use tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage;
    use tetra_pdus::umac::enums::access_assign_ul_usage::AccessAssignUlUsage;
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(multislot_config(false, 2));
    grant_pdch_slots(&mac.test.config, ISSI, &[4, 3], true);
    for ts in [3u8, 4] {
        let slots = mac.run_slots(8, ts);
        for s in &slots[1..] {
            let (header, aach) = aach_of(s);
            assert_eq!(
                (header, aach.dl_usage, aach.ul_usage),
                (2, AccessAssignDlUsage::AssignedControl, AccessAssignUlUsage::AssignedOnly),
                "ts {ts}"
            );
            assert_eq!(
                s.blk1.as_ref().unwrap().logical_channel,
                tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF
            );
        }
    }
    let ts2 = mac.run_slots(8, 2);
    assert!(ts2.iter().all(|s| aach_of(s).1.dl_usage == AccessAssignDlUsage::Unallocated));

    {
        let mut state = mac.test.config.state_write();
        state.timeslot_alloc.reserve(TimeslotOwner::Cmce, 2).unwrap();
        assert_eq!(state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).map(|s| s.ts), Some(3));
    }
    let ts4 = mac.run_slots(4, 4);
    assert!(
        ts4.iter().all(|s| aach_of(s).1.dl_usage == AccessAssignDlUsage::Unallocated),
        "the whole channel goes with the slot voice took"
    );
}

/// A group PDU reaches a member on its channel of several slots once, on one slot of it, and
/// once on the MCCH.
#[test]
fn a_group_call_setup_reaches_a_multislot_member_once() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(multislot_config(false, 2));
    {
        let mut state = mac.test.config.state_write();
        state.subscribers.register(ISSI);
        state.subscribers.affiliate(ISSI, GSSI);
    }
    grant_pdch_slots(&mac.test.config, ISSI, &[4, 3], true);
    mac.run(4);
    mac.test.submit_message(tl_data_to(TetraAddress::new(GSSI, SsiType::Gssi), false));
    let blocks = mac.run(4 * 6);
    let mut on: Vec<u8> = blocks
        .iter()
        .filter(|b| b.1 == MAIN_CARRIER && b.3.iter().any(|r| r.0 == GSSI))
        .map(|b| b.2)
        .collect();
    on.sort_unstable();
    assert_eq!(on.len(), 2, "{on:?}");
    assert_eq!(on[0], 1, "on the MCCH");
    assert!((3..=4).contains(&on[1]), "and once on the channel: {on:?}");
}

/// The dashboard sees every slot of the channel: PDCH active and inactive on each, and the
/// downlink to the radio on each (they all carry it).
#[test]
fn the_dashboard_shows_every_slot_of_a_multislot_pdch() {
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};
    let (sink, source) = telemetry_channel();
    let mut test = ComponentTest::from_config(multislot_config(false, 2), Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Llc, TetraEntity::Lmac]);
    test.register_entity(tetra_entities::umac::umac_bs::UmacBs::new(test.get_shared_config(), Some(sink)));
    let slots = grant_pdch_slots(&test.config, ISSI, &[4, 3], true);
    test.run_stack(Some(2));
    let changed = |events: &[TelemetryEvent], active: bool| -> Vec<u8> {
        let mut ts: Vec<u8> = events
            .iter()
            .filter_map(|e| match e {
                TelemetryEvent::PdchChanged {
                    ts, issi: ISSI, active: a, ..
                } if *a == active => Some(*ts),
                _ => None,
            })
            .collect();
        ts.sort_unstable();
        ts
    };
    let onto = pdch_telemetry(&source);
    assert_eq!(changed(&onto, true), vec![3, 4]);

    let mut pdu = BitBuffer::new_autoexpand(32);
    BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    append_bits(&mut pdu, "0101010101");
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            carrier_num: Some(MAIN_CARRIER),
            req_handle: 0,
            pdu,
            main_address: TetraAddress::issi(ISSI),
            link_id: 4,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let mut down: Vec<u8> = pdch_telemetry(&source)
        .iter()
        .filter_map(|e| match e {
            TelemetryEvent::TsDataActivity {
                ts,
                issi: ISSI,
                uplink: false,
                ..
            } => Some(*ts),
            _ => None,
        })
        .collect();
    down.sort_unstable();
    assert_eq!(down, vec![3, 4]);

    {
        let mut state = test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for s in slots {
            state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, s).unwrap();
        }
    }
    test.run_stack(Some(1));
    assert_eq!(changed(&pdch_telemetry(&source), false), vec![3, 4]);
}

/// Two radios never share a slot: the second gets what the first left, and a slot two channels
/// claim in the shared state (a stale entry) stays with the first ISSI.
#[test]
fn two_radios_never_share_a_slot() {
    let mut cfg = multislot_config(false, 2);
    cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    let mut air = Air::new(cfg);
    assert_eq!(onto_channel(&mut air, ISSI, &transmit_request_full(1, 4, 4)), vec![3, 4]);
    assert_eq!(onto_channel(&mut air, ISSI2, &transmit_request_full(1, 4, 4)), vec![2]);

    let mut test = ComponentTest::from_config(multislot_config(false, 2), Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Umac], vec![TetraEntity::Llc, TetraEntity::Lmac]);
    grant_pdch_slots(&test.config, ISSI, &[4, 3], true);
    {
        let mut state = test.config.state_write();
        let slot = state.timeslot_alloc.reserve_packet_data_slot(&[2]).unwrap();
        state.pdch_by_issi.insert(ISSI2, PdchGrant { slot, on_air: true });
        state.pdch_timeslots_by_issi.insert(ISSI2, [false, true, true, false]);
    }
    test.run_stack(Some(2));
    let umac = test
        .router
        .get_entity(TetraEntity::Umac)
        .and_then(|e| e.as_any_mut().downcast_mut::<tetra_entities::umac::umac_bs::UmacBs>())
        .expect("the UMAC");
    let owners: Vec<Option<u32>> = (2..=4).map(|ts| umac.channel_scheduler.pdch_owner(ts)).collect();
    assert_eq!(owners, vec![Some(ISSI2), Some(ISSI), Some(ISSI)]);
}

// ---------------------------------------------------------------------------------------------
// Packet-data carrier (`pdch_carrier`): channels of up to four slots on the secondary carrier
// ---------------------------------------------------------------------------------------------

/// `multislot_config` with the secondary carrier as the packet-data carrier.
fn carrier_config(wap: bool, n: u8, exclusive: bool) -> StackConfig {
    let mut cfg = multislot_config(wap, n);
    cfg.cell.secondary_carrier = Some(SECONDARY_CARRIER);
    cfg.packet_data.pdch_carrier = Some(SECONDARY_CARRIER);
    cfg.packet_data.pdch_carrier_exclusive = exclusive;
    cfg
}

fn carrier_slot(ts: u8) -> CarrierSlot {
    CarrierSlot {
        carrier_num: SECONDARY_CARRIER,
        ts,
    }
}

/// Give `issi` the channel of `slots` of the packet-data carrier (the first one its grant's) in
/// the shared state, as the SNDCP runtime does.
fn grant_carrier_slots(config: &SharedConfig, issi: u32, slots: &[u8], on_air: bool) -> Vec<CarrierSlot> {
    let mut state = config.state_write();
    let reserved = state
        .timeslot_alloc
        .reserve_packet_data_slots_on(SECONDARY_CARRIER, slots, slots.len());
    assert_eq!(reserved.len(), slots.len(), "slots free");
    state.pdch_by_issi.insert(issi, PdchGrant { slot: reserved[0], on_air });
    if reserved.len() > 1 {
        let mut bits = [false; 4];
        for s in &reserved {
            bits[usize::from(s.ts) - 1] = true;
        }
        state.pdch_timeslots_by_issi.insert(issi, bits);
    }
    reserved
}

fn tma_ind_on(issi: u32, pdu: BitBuffer, carrier: u16, ts: u32) -> SapMsg {
    let mut msg = tma_ind(issi, pdu, ts);
    if let SapMsgInner::TmaUnitdataInd(ind) = &mut msg.msg {
        ind.carrier_num = carrier;
    }
    msg
}

/// `from_lmac` on `carrier`, with the burst's RSSI.
fn from_lmac_on(pdu: BitBuffer, carrier: u16, rssi_dbfs: f32) -> SapMsg {
    let mut msg = from_lmac(pdu);
    if let SapMsgInner::TmvUnitdataInd(ind) = &mut msg.msg {
        ind.carrier_num = carrier;
        ind.rssi_dbfs = rssi_dbfs;
    }
    msg
}

/// A MAC-DATA of `issi` in the clear with a TM-SDU of 24 bits (8 octets with its fill bits),
/// in a full uplink slot.
fn mac_data_of(issi: u32) -> BitBuffer {
    let mut b = format!("001000{:024b}0{:06b}{}", issi, 8, "1011".repeat(6));
    b.push('1');
    while b.len() < 268 {
        b.push('0');
    }
    let mut buf = BitBuffer::from_bitstr(&b);
    buf.seek(0);
    buf
}

impl Air {
    /// An LLC PDU from `issi` on timeslot `ts` of the packet-data carrier.
    fn uplink_on_carrier(&mut self, issi: u32, llc_pdu: BitBuffer, ts: u8) {
        while self.ticks % 4 != (usize::from(ts) + 1) % 4 {
            self.step();
        }
        self.test
            .submit_message(tma_ind_on(issi, llc_pdu, SECONDARY_CARRIER, u32::from(ts)));
        self.step();
    }

    /// An SN-PDU on the acknowledged basic link on timeslot `ts` of the packet-data carrier.
    fn send_on_carrier(&mut self, issi: u32, sn: &str, ts: u8) {
        let ns = self.ns.entry(issi).or_insert(0);
        let mut pdu = BitBuffer::new_autoexpand(64);
        BlData { has_fcs: false, ns: *ns }.to_bitbuf(&mut pdu);
        *ns ^= 1;
        append_bits(&mut pdu, &format!("100{sn}"));
        self.uplink_on_carrier(issi, pdu, ts);
    }
}

/// The radio's first uplink on its channel of the packet-data carrier: its AL-SETUP on `ts`.
fn al_setup_on_carrier(air: &mut Air, issi: u32, ts: u8) {
    air.uplink_on_carrier(issi, al_pdu(|b| radio_al_setup().to_bitbuf(b)), ts);
}

/// PDP context and the TRANSMIT REQUEST of a radio that can take 4 slots: the channel assigned
/// (carrier, timeslots), None when the data stays on the MCCH.
fn onto_carrier(air: &mut Air, issi: u32) -> Option<(Option<u16>, Vec<u8>)> {
    air.send(issi, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.send(issi, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        decode_data_transmit_response(&response.sn_buf()).unwrap().result,
        SndcpDataTransmitResponseResult::Accepted
    );
    air.run(2);
    air.take_sn();
    response.alloc.as_ref().map(|a| (a.carrier, ts_of(a)))
}

/// A radio that can take 4 slots is sent to the packet-data carrier: the RESPONSE on the MCCH
/// with a channel allocation Replace, both ways, carrier 1522, all four slots; the grant on ts4
/// (the first of ts 2-4 preferred), the main carrier's traffic slots left to voice.
#[test]
fn a_request_is_assigned_the_packet_data_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(response.link_id, 0, "sent on the MCCH, where the radio still is");
    assert_eq!(response.category, Some(DATA_CATEGORY_PDCH_ASSIGNMENT));
    let alloc = response.alloc.clone().expect("an assignment");
    assert_eq!(
        (alloc.alloc_type, alloc.ul_dl_assigned, alloc.carrier, alloc.timeslots),
        (ChanAllocType::Replace, UlDlAssignment::Both, Some(SECONDARY_CARRIER), [true; 4])
    );
    let state = air.test.config.state_read();
    assert_eq!(state.pdch_by_issi[&ISSI].slot, carrier_slot(4));
    assert_eq!(state.pdch_timeslots_by_issi.get(&ISSI), Some(&[true; 4]));
    for ts in 1..=4 {
        assert_eq!(state.timeslot_alloc.slot_owner(carrier_slot(ts)), Some(TimeslotOwner::PacketData));
    }
    assert!(
        (2..=4).all(|ts| state.timeslot_alloc.is_free(ts)),
        "main traffic slots left to voice"
    );
}

/// The width on the carrier: `pdch_max_slots`, what is free there (never ts1 alone), one slot
/// left for voice when the main carrier is full (shared), none left when data only.
#[test]
fn the_width_on_the_carrier() {
    let case = |max: u8, exclusive: bool, busy: &[CarrierSlot], full: u8| {
        let mut air = Air::new(carrier_config(false, max, exclusive));
        for slot in busy {
            air.test
                .config
                .state_write()
                .timeslot_alloc
                .reserve_slot(TimeslotOwner::Cmce, *slot)
                .unwrap();
        }
        air.send(ISSI, &demand(1, None, false));
        air.next_sn(8).expect("ACCEPT");
        air.send(ISSI, &transmit_request_full(1, full, full));
        let response = air.next_sn(8).expect("RESPONSE");
        let grant = air.test.config.state_read().pdch_by_issi.get(&ISSI).map(|g| g.slot);
        (response.alloc.map(|a| (a.carrier, ts_of(&a))), grant)
    };
    let main = |ts: u8| CarrierSlot {
        carrier_num: MAIN_CARRIER,
        ts,
    };
    let main_full = [main(2), main(3), main(4)];
    assert_eq!(
        case(2, false, &[], 4),
        (Some((Some(SECONDARY_CARRIER), vec![3, 4])), Some(carrier_slot(4)))
    );
    assert_eq!(
        case(4, false, &main_full, 4).0,
        Some((Some(SECONDARY_CARRIER), vec![2, 3, 4])),
        "voice headroom"
    );
    assert_eq!(
        case(4, true, &main_full, 4).0,
        Some((Some(SECONDARY_CARRIER), vec![1, 2, 3, 4])),
        "data only"
    );
    assert_eq!(
        case(4, false, &[carrier_slot(4)], 4),
        (Some((Some(SECONDARY_CARRIER), vec![1, 2, 3])), Some(carrier_slot(3)))
    );
    assert_eq!(case(4, false, &[], 1).0, Some((Some(SECONDARY_CARRIER), vec![4])));
    assert_eq!(
        case(4, false, &[carrier_slot(2), carrier_slot(3), carrier_slot(4)], 4).0,
        Some((Some(MAIN_CARRIER), vec![2, 3, 4])),
        "never ts1 alone: the main carrier"
    );
}

/// The carrier or the main carrier, whichever gives more slots; the carrier on a tie.
#[test]
fn a_busy_carrier_falls_back_to_the_main_pdch_timeslots() {
    let case = |busy: &[CarrierSlot]| {
        let mut air = Air::new(carrier_config(false, 2, false));
        for slot in busy {
            air.test
                .config
                .state_write()
                .timeslot_alloc
                .reserve_slot(TimeslotOwner::Cmce, *slot)
                .unwrap();
        }
        onto_carrier(&mut air, ISSI)
    };
    let main2 = CarrierSlot {
        carrier_num: MAIN_CARRIER,
        ts: 2,
    };
    let all = [carrier_slot(1), carrier_slot(2), carrier_slot(3), carrier_slot(4)];
    assert_eq!(case(&all), Some((Some(MAIN_CARRIER), vec![3, 4])), "carrier full of voice");
    assert_eq!(
        case(&[main2, carrier_slot(1), carrier_slot(2), carrier_slot(4)]),
        Some((Some(MAIN_CARRIER), vec![3, 4])),
        "one there, two on the main carrier"
    );
    assert_eq!(
        case(&[main2, carrier_slot(1), carrier_slot(2)]),
        Some((Some(SECONDARY_CARRIER), vec![3, 4])),
        "a tie: the carrier"
    );
}

/// A radio on its carrier channel gets its PDUs there: individual signalling (MM following the
/// uplink too), SN-UNITDATA, and the BL-ACK of an uplink on a slot of the channel; never stolen.
#[test]
fn pdus_follow_the_radio_to_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), true));
    air.test.submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 10, None));
    air.run(40);
    let mut data = BitBuffer::new_autoexpand(32);
    BlData { has_fcs: false, ns: 0 }.to_bitbuf(&mut data);
    append_bits(&mut data, "0101010101");
    air.uplink_on_carrier(ISSI, data, 3);
    air.run(2);
    let mine: Vec<&Down> = air.down.iter().filter(|d| d.issi == ISSI).collect();
    assert!(mine.len() >= 4, "{mine:?}");
    for d in &mine {
        assert_eq!((d.carrier, d.stealing), (Some(SECONDARY_CARRIER), false), "{d:?}");
    }
    assert_eq!(mine.iter().filter(|d| d.link_id == 4).count(), mine.len() - 1, "{mine:?}");
    assert!(
        mine.iter().any(|d| d.llc == LlcPduType::BlAck && d.link_id == 3),
        "the BL-ACK on the slot of the uplink: {mine:?}"
    );
}

/// Data for a radio on its channel of the packet-data carrier goes out there, never on the main
/// carrier. (miura: in place of tea2's test of the carrier's ECK, no encryption here.)
#[test]
fn the_data_on_the_carrier_goes_out_there() {
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(carrier_config(false, 4, false));
    grant_carrier_slots(&mac.test.config, ISSI, &[4, 3, 2, 1], true);
    mac.run(4);
    mac.test.submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 120, None));
    mac.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    let mine: Vec<(u16, u8)> = mac
        .run(4 * 30)
        .iter()
        .filter(|b| b.3.iter().any(|r| r.0 == ISSI))
        .map(|b| (b.1, b.2))
        .collect();
    assert!(mine.len() >= 2, "{mine:?}");
    assert!(mine.iter().all(|m| m.0 == SECONDARY_CARRIER), "{mine:?}");
}

/// A TM-SDU of 400 bits sent up on the radio's carrier channel {1,2,3,4}: MAC-DATA opening the
/// fragmentation on ts3 asking for 2 slots, then MAC-FRAG and MAC-END in the slots the grant
/// gives (read as the MS does, ts1 counted).
fn uplink_on_the_carrier(cfg: StackConfig) -> (Vec<TmaUnitdataInd>, Vec<TdmaTime>, String) {
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    let mut air = UmacAir::new(cfg);
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    let sdu: String = (0..400).map(|i| if (i * 7) % 3 == 0 { '1' } else { '0' }).collect();
    let (first, frag, end) = (&sdu[..150], &sdu[150..300], &sdu[300..]);
    let start = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let header = format!("001000{:024b}11{:04b}0", ISSI, ReservationRequirement::Req2Slots as u64);
    let on_carrier = |b: BitBuffer| from_lmac_on(b, SECONDARY_CARRIER, -40.0);
    air.uplink_at(start, on_carrier(uplink_block(&header, first)));
    let (at, slots, delay) = air.next_grant(ISSI, 16);
    assert_eq!(slots, 2);
    let labels = ms_granted_labels(at, delay, slots, &[1, 2, 3, 4]);
    assert!(labels.iter().all(|l| l.t != 1), "never uplink ts1 of the carrier: {labels:?}");
    air.uplink_at(labels[0], on_carrier(uplink_block("0101", frag)));
    let end_octets = (10 + end.len() + 1).div_ceil(8);
    air.uplink_at(labels[1], on_carrier(uplink_block(&format!("0111{end_octets:06b}"), end)));
    for _ in 0..4 {
        air.tick();
    }
    (air.up, labels, sdu)
}

/// Uplink on the carrier: granted over its channel, reassembled across its slots; up once, from
/// the radio, on the carrier.
#[test]
fn uplink_on_the_carrier_is_reassembled() {
    debug::setup_logging_verbose();
    let (up, labels, sdu) = uplink_on_the_carrier(carrier_config(false, 4, false));
    assert_eq!(up.len(), 1, "{up:?}");
    assert_eq!(
        (
            up[0].carrier_num,
            up[0].main_address.ssi,
            up[0].link_id,
            up[0].pdu.as_ref().unwrap().to_bitstr()
        ),
        (SECONDARY_CARRIER, ISSI, u32::from(labels[1].t), sdu)
    );
}

/// A MAC-DATA of the radio on uplink ts1 of the carrier is not its (its channel never transmits
/// there): dropped as an adjacent-channel copy; on ts3 it is delivered. The same for a
/// MAC-U-BLCK, which names no radio and would otherwise be taken for the channel's.
#[test]
fn no_uplink_on_carrier_ts1() {
    let up = |ts: u8, msg: SapMsg| {
        let mut air = UmacAir::new(carrier_config(false, 4, false));
        grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
        let label = TdmaTime { t: ts, f: 3, m: 1, h: 0 };
        air.uplink_at(label, msg);
        air.tick();
        air.up
    };
    let mac_data = || from_lmac_on(mac_data_of(ISSI), SECONDARY_CARRIER, -40.0);
    assert!(up(1, mac_data()).is_empty(), "ts1: dropped");
    assert_eq!(up(3, mac_data()).len(), 1, "ts3: delivered");
    let u_blck = || {
        let mut msg = mac_u_blck(5, 15);
        if let SapMsgInner::TmvUnitdataInd(ind) = &mut msg.msg {
            ind.carrier_num = SECONDARY_CARRIER;
        }
        msg
    };
    assert!(up(1, u_blck()).is_empty(), "MAC-U-BLCK on ts1: dropped");
    let up3 = up(3, u_blck());
    assert_eq!(
        up3.iter().map(|u| (u.main_address.ssi, u.link_id)).collect::<Vec<_>>(),
        vec![(ISSI, 3)],
        "MAC-U-BLCK on ts3: the channel's"
    );
}

/// A group PDU goes on the MCCH and, for each member on a channel, a copy there: on the carrier
/// for a member there (its channel allocation keeping the original's carrier), on the main
/// carrier for a member there.
#[test]
fn a_group_call_setup_copy_reaches_a_member_on_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    grant_pdch(&air.test.config, ISSI2, 3, true);
    {
        let mut state = air.test.config.state_write();
        for issi in [ISSI, ISSI2] {
            state.subscribers.register(issi);
            state.subscribers.affiliate(issi, GSSI);
        }
    }
    let mut msg = tl_data_to(TetraAddress::new(GSSI, SsiType::Gssi), false);
    if let SapMsgInner::TlaTlDataReqBl(req) = &mut msg.msg {
        req.chan_alloc = Some(CmceChanAllocReq {
            usage: Some(4),
            carrier: None,
            timeslots: [false, true, false, false],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        });
    }
    air.test.submit_message(msg);
    air.run(2);
    let mut to_group: Vec<(u32, Option<u16>, Option<u16>)> = air
        .down
        .iter()
        .filter(|d| d.issi == GSSI)
        .map(|d| (d.link_id, d.carrier, d.alloc.as_ref().and_then(|a| a.carrier)))
        .collect();
    to_group.sort_unstable();
    assert_eq!(
        to_group,
        vec![
            (0, Some(MAIN_CARRIER), None),
            (3, Some(MAIN_CARRIER), Some(MAIN_CARRIER)),
            (4, Some(SECONDARY_CARRIER), Some(MAIN_CARRIER)),
        ]
    );
}

/// A call that takes a slot of a member's carrier channel (main full, shared): no copy on the
/// channel it is losing, the D-SETUP on the MCCH, and the same D-SETUP again on the MCCH four
/// frames later, when the member has left the channel; its channel goes.
#[test]
fn a_call_that_takes_the_carrier_channel_still_reaches_its_member() {
    debug::setup_logging_verbose();
    let mut cfg = voice_config(Some(PacketDataBearer::Pdch), true);
    cfg.packet_data.pdch_carrier = Some(SECONDARY_CARRIER);
    let mut v = Voice::new(cfg, false);
    v.register(DATA_RADIO, Some(104));
    grant_carrier_slots(&v.test.config, DATA_RADIO, &[4, 3, 2, 1], true);
    let opens = three_group_calls(&mut v, false);
    assert_eq!(opens, vec![(MAIN_CARRIER, 2), (MAIN_CARRIER, 3), (MAIN_CARRIER, 4)]);
    v.msgs.clear();
    let mut seen: Vec<(usize, u32, String)> = Vec::new();
    v.cmce(3_000_004, u_setup(104, true, false, 0));
    for _ in 0..30 {
        for m in v.msgs.drain(..) {
            if let SapMsgInner::TmaUnitdataReq(req) = &m.msg
                && req.main_address.ssi == 104
            {
                seen.push((v.ticks, req.link_id, req.pdu.to_bitstr()));
            }
        }
        v.run(1);
    }
    assert!(seen.iter().all(|s| s.1 == 0), "nothing on the channel the call took: {seen:?}");
    let first = seen.first().expect("the D-SETUP").clone();
    assert!(
        seen.iter().any(|s| s.2 == first.2 && s.0 >= first.0 + 16),
        "the same PDU again on the MCCH 4 frames later: {seen:?}"
    );
    assert!(
        v.test.config.state_read().pdch_channel(DATA_RADIO).is_none(),
        "its channel went to the call"
    );
}

/// A call took the member's carrier channel and the SNDCP has not given the channel back yet: a
/// group PDU of the first four frames is repeated on the MCCH (the member may still be on its
/// channel), a later one is not (the member hears the MCCH by then, and the members there would
/// get it twice).
#[test]
fn only_the_group_pdus_just_after_a_preemption_are_repeated() {
    debug::setup_logging_verbose();
    let mut test = ComponentTest::from_config(carrier_config(false, 4, false), Some(TdmaTime::default()));
    test.populate_entities(vec![TetraEntity::Llc], vec![TetraEntity::Umac, TetraEntity::Mle]);
    grant_carrier_slots(&test.config, ISSI, &[4, 3, 2, 1], true);
    {
        let mut state = test.config.state_write();
        state.subscribers.register(ISSI);
        state.subscribers.affiliate(ISSI, GSSI);
        for ts in 2..=4 {
            state.timeslot_alloc.reserve(TimeslotOwner::Cmce, ts).unwrap();
        }
        assert_eq!(state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce), Some(carrier_slot(1)));
    }
    let mut to_group = Vec::new();
    for tick in 0..80 {
        if tick == 1 || tick == 30 {
            test.submit_message(tl_data_to(TetraAddress::new(GSSI, SsiType::Gssi), false));
        }
        test.run_stack(Some(1));
        for m in test.dump_sinks() {
            if let SapMsgInner::TmaUnitdataReq(req) = &m.msg
                && req.main_address.ssi == GSSI
            {
                to_group.push((tick, req.link_id));
            }
        }
    }
    assert!(test.config.state_read().pdch_preempted(ISSI), "the channel not given back yet");
    assert!(to_group.iter().all(|g| g.1 == 0), "all on the MCCH: {to_group:?}");
    let ticks: Vec<usize> = to_group.iter().map(|g| g.0).collect();
    assert_eq!(ticks.len(), 3, "the first twice, the second once: {ticks:?}");
    assert!(ticks[1] >= ticks[0] + 16 && ticks[2] >= 30, "{ticks:?}");
}

/// The radio's BL-ACK of its assignment is lost; its first uplink on the channel (its AL-SETUP)
/// acknowledges it: no retransmission on the MCCH, no "never acknowledged" release, and the
/// next acknowledged PDU to it leaves on the carrier at once.
#[test]
fn the_assignment_is_acknowledged_by_the_first_uplink_on_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.run(4);
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let mut responses = 0;
    for _ in 0..3 {
        air.acks.retain(|a| a.0 != ISSI);
        responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
        air.step();
    }
    air.acks.retain(|a| a.0 != ISSI);
    responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
    assert_eq!(responses, 1, "the RESPONSE went");
    al_setup_on_carrier(&mut air, ISSI, 4);
    for _ in 0..200 {
        air.step();
        responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
    }
    assert_eq!(responses, 1, "not sent again");
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air, "the channel is kept");
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.run(2);
    let d = air.down.iter().find(|d| d.issi == ISSI).expect("a PDU went down");
    assert_eq!((d.carrier, d.link_id), (Some(SECONDARY_CARRIER), 4));
}

/// A radio taken off its carrier channel by a false or stale MCCH flag is put back on it by its
/// next uplink there: its TRANSMIT REQUEST on the carrier is "already on its PDCH", and its
/// group's PDUs are copied to its channel again.
#[test]
fn a_false_mcch_flag_is_undone_by_the_next_uplink_on_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    assert_eq!(onto_carrier(&mut air, ISSI).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
    al_setup_on_carrier(&mut air, ISSI, 4);
    air.run(8);
    air.test.config.state_write().pdch_by_issi.get_mut(&ISSI).unwrap().on_air = false;
    air.run(4);
    air.take_sn();
    air.send_on_carrier(ISSI, &transmit_request_full(1, 4, 4), 3);
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        (response.carrier, response.link_id, response.chan_alloc),
        (Some(SECONDARY_CARRIER), 4, false),
        "already on its PDCH, answered there"
    );
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
    {
        let mut state = air.test.config.state_write();
        state.subscribers.register(ISSI);
        state.subscribers.affiliate(ISSI, GSSI);
    }
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::new(GSSI, SsiType::Gssi), false));
    air.run(2);
    assert!(
        air.down
            .iter()
            .any(|d| d.issi == GSSI && d.carrier == Some(SECONDARY_CARRIER) && d.link_id == 4),
        "the copy on its channel"
    );
}

/// A radio that stays on the MCCH after its carrier assignment (it acknowledges it there, then
/// sends an SN-PDU there before anything on its channel) loses the channel and gets main-carrier
/// channels from then on; another radio still gets the carrier.
#[test]
fn a_radio_that_does_not_move_gets_the_main_carrier_from_then_on() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    assert_eq!(onto_carrier(&mut air, ISSI).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        response.alloc.as_ref().map(|a| a.carrier),
        Some(Some(MAIN_CARRIER)),
        "main carrier from now on"
    );
    assert_eq!(air.test.config.state_read().pdch_by_issi[&ISSI].slot.carrier_num, MAIN_CARRIER);
    air.run(80);
    assert_eq!(
        onto_carrier(&mut air, ISSI2).map(|a| a.0),
        Some(Some(SECONDARY_CARRIER)),
        "another radio: the carrier"
    );
}

/// The block with the radio's carrier assignment is lost on the air: the radio, still waiting for
/// its acknowledgement, sends its TRANSMIT REQUEST again on the MCCH. It never refused the
/// carrier: the channel is kept, the assignment reaches it on the MCCH, and once the radio is on
/// its channel its PDUs go there.
#[test]
fn a_lost_assignment_does_not_bar_the_radio_from_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(false, 4, false));
    air.send(ISSI, &demand(1, None, false));
    air.next_sn(8).expect("ACCEPT");
    air.run(4);
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let mut responses = 0;
    for _ in 0..3 {
        air.acks.retain(|a| a.0 != ISSI);
        responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
        air.step();
    }
    air.acks.retain(|a| a.0 != ISSI);
    responses += air.take_sn().iter().filter(|d| d.sn_type() == Some(7)).count();
    assert_eq!(responses, 1, "the RESPONSE went");
    // The same BL-DATA again (same N(S)): the radio never got the answer.
    *air.ns.get_mut(&ISSI).unwrap() ^= 1;
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    air.run(40);
    assert_eq!(
        air.test.config.state_read().pdch_by_issi.get(&ISSI).map(|g| g.slot.carrier_num),
        Some(SECONDARY_CARRIER),
        "the carrier channel is kept"
    );
    al_setup_on_carrier(&mut air, ISSI, 4);
    air.run(8);
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air, "on its channel");
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.run(2);
    let d = air.down.iter().find(|d| d.issi == ISSI).expect("a PDU went down");
    assert_eq!((d.carrier, d.link_id), (Some(SECONDARY_CARRIER), 4));
}

/// The radio back on the MCCH (a TRANSMIT REQUEST there, after it was heard on its channel) is
/// off its channel, which it is assigned again; a request on a slot of its channel is answered
/// there.
#[test]
fn the_radio_back_on_the_mcch_is_flagged_by_link_1() {
    let mut air = Air::new(carrier_config(false, 4, false));
    assert_eq!(onto_carrier(&mut air, ISSI).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
    al_setup_on_carrier(&mut air, ISSI, 4);
    air.run(8);
    air.take_sn();
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let again = air.next_sn(8).expect("RESPONSE");
    assert_eq!(again.link_id, 0, "on the MCCH");
    assert_eq!(
        again.alloc.as_ref().map(|a| (a.carrier, ts_of(a))),
        Some((Some(SECONDARY_CARRIER), vec![1, 2, 3, 4])),
        "assigned again"
    );
    air.run(2);
    air.take_sn();
    air.send_on_carrier(ISSI, &transmit_request_full(1, 4, 4), 3);
    let there = air.next_sn(8).expect("RESPONSE");
    assert_eq!(
        (there.carrier, there.link_id, there.chan_alloc),
        (Some(SECONDARY_CARRIER), 4, false)
    );
}

/// A BL-ACK of the radio on the MCCH (where it may acknowledge its assignment) does not take it
/// off its carrier channel once it was heard there: its PDUs still go to the carrier.
#[test]
fn a_bl_ack_on_the_mcch_does_not_take_it_off_air() {
    let mut air = Air::new(carrier_config(false, 4, false));
    assert_eq!(onto_carrier(&mut air, ISSI).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
    al_setup_on_carrier(&mut air, ISSI, 4);
    air.run(8);
    let mut ack = BitBuffer::new_autoexpand(8);
    BlAck { has_fcs: false, nr: 0 }.to_bitbuf(&mut ack);
    ack.seek(0);
    air.test.submit_message(tma_ind(ISSI, ack, 1));
    air.run(8);
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
    air.down.clear();
    air.test.submit_message(tl_data_to(TetraAddress::issi(ISSI), false));
    air.run(2);
    let d = air.down.iter().find(|d| d.issi == ISSI).expect("a PDU went down");
    assert_eq!((d.carrier, d.link_id), (Some(SECONDARY_CARRIER), 4));
}

/// Idle for `pdch_idle_release_secs`: SN-END OF DATA with "quit and go" to the main carrier on the
/// carrier channel; its slots free, and given to a channel again only after a multiframe.
#[test]
fn an_idle_carrier_pdch_ends_with_end_of_data_on_the_carrier() {
    debug::setup_logging_verbose();
    let mut cfg = carrier_config(false, 4, false);
    cfg.packet_data.ready_timer_code = 11;
    cfg.packet_data.pdch_idle_release_secs = 2;
    let mut air = Air::new(cfg);
    assert_eq!(onto_carrier(&mut air, ISSI).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
    // Until the quit went out and the channel was given back.
    for _ in 0..400 {
        air.step();
        if air.test.config.state_read().pdch_by_issi.is_empty() {
            break;
        }
    }
    let sent = air.take_sn();
    let eod: Vec<&Down> = sent.iter().filter(|d| d.sn_type() == Some(8)).collect();
    assert_eq!(eod.len(), 1, "{sent:?}");
    assert_eq!((eod[0].carrier, eod[0].link_id), (Some(SECONDARY_CARRIER), 4), "on its channel");
    let quit = eod[0].alloc.clone().expect("quit");
    assert_eq!(
        (quit.alloc_type, quit.carrier, ts_of(&quit)),
        (ChanAllocType::QuitAndGo, Some(MAIN_CARRIER), vec![])
    );
    {
        let state = air.test.config.state_read();
        assert!(state.pdch_by_issi.is_empty());
        assert!((1..=4).all(|ts| state.timeslot_alloc.slot_is_free(carrier_slot(ts))));
    }
    assert_eq!(
        onto_carrier(&mut air, ISSI2).map(|a| a.0),
        Some(Some(MAIN_CARRIER)),
        "the carrier's slots wait a multiframe"
    );
    air.run(80);
    assert_eq!(onto_carrier(&mut air, 3_000_001).map(|a| a.0), Some(Some(SECONDARY_CARRIER)));
}

/// Voice taking a slot of a carrier channel (nothing else free) takes the whole channel: the PDU
/// queued for its radio is dropped at once, and its other slots show "unallocated" (SCH/F) so the
/// radio leaves (N.208).
#[test]
fn voice_taking_a_carrier_slot_releases_the_whole_channel() {
    use tetra_core::Direction;
    use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
    debug::setup_logging_verbose();
    let mut mac = MacAir::with(carrier_config(false, 4, false));
    grant_carrier_slots(&mac.test.config, ISSI, &[4, 3, 2, 1], true);
    mac.run(4);
    let reporter = TxReporter::new_unacked();
    mac.test
        .submit_message(tl_unitdata_to(TetraAddress::issi(ISSI), 2000, Some(reporter.clone())));
    mac.run(2);
    let taken = {
        let mut state = mac.test.config.state_write();
        for ts in 2..=4 {
            state.timeslot_alloc.reserve(TimeslotOwner::Cmce, ts).unwrap();
        }
        state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("a carrier slot")
    };
    assert_eq!(taken, carrier_slot(1));
    mac.test.submit_message(SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(CallControl::Open(Circuit {
            direction: Direction::Both,
            carrier_num: SECONDARY_CARRIER,
            ts: 1,
            peer_carrier_num: None,
            peer_ts: None,
            usage: 6,
            circuit_mode: CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::SwMI,
        })),
    });
    let mut after = Vec::new();
    for _ in 0..8 {
        mac.test.run_stack(Some(1));
        for msg in mac.test.dump_sinks() {
            if let SapMsgInner::TmvUnitdataReqSlots(slots) = msg.msg {
                after.extend(
                    slots
                        .slots
                        .into_iter()
                        .filter(|s| s.carrier_num == SECONDARY_CARRIER && s.ts.f != 18),
                );
            }
        }
    }
    assert_eq!(reporter.get_state(), TxState::Discarded);
    let others: Vec<_> = after.iter().skip(4).filter(|s| s.ts.t != 1).collect();
    assert!(!others.is_empty());
    for s in others {
        assert_eq!(
            aach_of(s).1.dl_usage,
            tetra_pdus::umac::enums::access_assign_dl_usage::AccessAssignDlUsage::Unallocated,
            "{}",
            s.ts
        );
        assert_eq!(
            s.blk1.as_ref().unwrap().logical_channel,
            tetra_saps::tmv::enums::logical_chans::LogicalChannel::SchF
        );
    }
}

/// Data only (`pdch_carrier_exclusive`): with the main carrier full a fourth call gets nothing on
/// the carrier, whose channel stays.
#[test]
fn the_exclusive_carrier_is_never_given_to_voice() {
    let mut cfg = voice_config(Some(PacketDataBearer::Pdch), true);
    cfg.packet_data.pdch_carrier = Some(SECONDARY_CARRIER);
    cfg.packet_data.pdch_carrier_exclusive = true;
    let mut v = Voice::new(cfg, false);
    grant_carrier_slots(&v.test.config, DATA_RADIO, &[4, 3], true);
    three_group_calls(&mut v, false);
    v.register(2_000_104, Some(104));
    v.cmce(3_000_004, u_setup(104, true, false, 0));
    v.run(40);
    let opens: Vec<(u16, u8)> = v
        .msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::CmceCallControl(tetra_saps::control::call_control::CallControl::Open(c)) => Some((c.carrier_num, c.ts)),
            _ => None,
        })
        .collect();
    assert!(opens.len() >= 3 && opens.iter().all(|o| o.0 == MAIN_CARRIER), "{opens:?}");
    assert!(v.test.config.state_read().pdch_channel(DATA_RADIO).is_some(), "the channel stays");
}

/// Data only, main carrier full of ordinary calls: an emergency call pre-empts one of them (the
/// carrier is not counted free), never the data channel.
#[test]
fn exclusive_mode_with_priority_preemption() {
    let mut cfg = voice_config(Some(PacketDataBearer::Pdch), true);
    cfg.packet_data.pdch_carrier = Some(SECONDARY_CARRIER);
    cfg.packet_data.pdch_carrier_exclusive = true;
    let mut v = Voice::new(cfg, false);
    grant_carrier_slots(&v.test.config, DATA_RADIO, &[4, 3, 2, 1], true);
    assert_eq!(v.test.config.state_read().timeslot_alloc.free_slot_count(), 3);
    for (i, g) in [101u32, 102, 103, 199].into_iter().enumerate() {
        v.register(2_000_001 + i as u32, Some(g));
    }
    for (i, g) in [101u32, 102, 103].into_iter().enumerate() {
        v.cmce(3_000_001 + i as u32, u_setup(g, true, false, 0));
        v.run(4);
    }
    v.run(40);
    assert_eq!(v.test.config.state_read().timeslot_alloc.free_slot_count(), 0);
    v.cmce(3_000_099, u_setup(199, true, false, 15));
    v.run(40);
    assert!(count(&v.log, "CallEnded") >= 1, "the emergency call pre-empted a call");
    assert!(
        v.test.config.state_read().pdch_channel(DATA_RADIO).is_some(),
        "the data channel stays"
    );
}

/// The carrier channel of `ISSI` given back (as after its quit went out), in a UMAC ticking.
fn released_carrier_channel() -> UmacAir {
    let mut air = UmacAir::new(carrier_config(false, 4, false));
    let slots = grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    for _ in 0..4 {
        air.tick();
    }
    {
        let mut state = air.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for s in slots {
            state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, s).unwrap();
        }
    }
    air.tick();
    air
}

/// The last bursts of a radio after its carrier channel went (the BL-ACK of its quit) are its own,
/// not adjacent-channel copies; another radio's there are. A quit sent again after the release
/// goes on the MCCH without its channel allocation.
#[test]
fn the_last_uplink_after_the_quit_is_accepted() {
    debug::setup_logging_verbose();
    for (who, expected) in [(ISSI, 1), (ISSI2, 0)] {
        let mut air = released_carrier_channel();
        let label = air.next_time().add_timeslots(8).forward_to_timeslot(4);
        air.uplink_at(label, from_lmac_on(mac_data_of(who), SECONDARY_CARRIER, -40.0));
        air.tick();
        assert_eq!(air.up.len(), expected, "ISSI {who}");
    }

    let mut air = released_carrier_channel();
    air.test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr("0000100110011000"),
            main_address: TetraAddress::issi(ISSI),
            link_id: 4,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: Some(SECONDARY_CARRIER),
            chan_alloc: Some(CmceChanAllocReq {
                usage: None,
                carrier: Some(MAIN_CARRIER),
                timeslots: [false; 4],
                alloc_type: ChanAllocType::QuitAndGo,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            tx_reporter: None,
        }),
    });
    for _ in 0..8 {
        air.tick();
    }
    let quit: Vec<(u16, u8, bool)> = air
        .slots
        .iter()
        .flat_map(|s| mac_resources(s).into_iter().map(move |r| (s.carrier_num, s.ts.t, r)))
        .filter(|(_, _, r)| r.addr.is_some_and(|a| a.ssi == ISSI))
        .map(|(c, t, r)| (c, t, r.chan_alloc_element.is_some()))
        .collect();
    assert_eq!(quit, vec![(MAIN_CARRIER, 1, false)], "on the MCCH, without the allocation");
}

/// A PDU with a channel allocation (here an assignment sent again, as the LLC routed it to the
/// channel) for a radio whose carrier channel went more than a multiframe ago goes on the MCCH,
/// where the radio is, not on that timeslot of the main carrier.
#[test]
fn an_allocation_for_a_carrier_channel_long_gone_goes_on_the_mcch() {
    debug::setup_logging_verbose();
    let mut air = released_carrier_channel();
    for _ in 0..80 {
        air.tick();
    }
    air.slots.clear();
    air.test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr("0000100110011000"),
            main_address: TetraAddress::issi(ISSI),
            link_id: 4,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            carrier_num: Some(SECONDARY_CARRIER),
            chan_alloc: Some(CmceChanAllocReq {
                usage: None,
                carrier: Some(SECONDARY_CARRIER),
                timeslots: [true; 4],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            tx_reporter: None,
        }),
    });
    for _ in 0..8 {
        air.tick();
    }
    let sent: Vec<(u16, u8)> = air
        .slots
        .iter()
        .flat_map(|s| mac_resources(s).into_iter().map(move |r| (s.carrier_num, s.ts.t, r)))
        .filter(|(_, _, r)| r.addr.is_some_and(|a| a.ssi == ISSI))
        .map(|(c, t, _)| (c, t))
        .collect();
    assert_eq!(sent, vec![(MAIN_CARRIER, 1)], "on the MCCH");
}

/// A carrier assignment waiting on the MCCH (behind another radio's PDU that fills a block) when
/// its channel goes (a call took a slot, or it was given back) goes out without its channel
/// allocation: the SN-DATA TRANSMIT RESPONSE still reaches the radio, which is not sent to a
/// carrier where it has no channel. With the channel still there it goes with it.
#[test]
fn a_queued_carrier_assignment_whose_channel_went_goes_without_it() {
    debug::setup_logging_verbose();
    let tma = |issi: u32, category: Option<i32>, chan_alloc: Option<CmceChanAllocReq>, bits: &str| SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr(bits),
            main_address: TetraAddress::issi(issi),
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: category,
            carrier_num: chan_alloc.as_ref().and_then(|c| c.carrier),
            chan_alloc,
            tx_reporter: None,
        }),
    };
    for goes in [false, true] {
        let mut air = UmacAir::new(carrier_config(false, 4, false));
        let slots = grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], false);
        air.tick();
        air.test.submit_message(tma(ISSI2, None, None, &"0010".repeat(52)));
        air.test.submit_message(tma(
            ISSI,
            Some(DATA_CATEGORY_PDCH_ASSIGNMENT),
            Some(CmceChanAllocReq {
                usage: None,
                carrier: Some(SECONDARY_CARRIER),
                timeslots: [true; 4],
                alloc_type: ChanAllocType::Replace,
                ul_dl_assigned: UlDlAssignment::Both,
            }),
            "0000100110011000",
        ));
        air.tick();
        if goes {
            let mut state = air.test.config.state_write();
            state.pdch_by_issi.remove(&ISSI);
            state.pdch_timeslots_by_issi.remove(&ISSI);
            for s in &slots {
                state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, *s).unwrap();
            }
        }
        for _ in 0..40 {
            air.tick();
        }
        let mine: Vec<(u16, u8, bool)> = air
            .slots
            .iter()
            .flat_map(|s| mac_resources(s).into_iter().map(move |r| (s.carrier_num, s.ts.t, r)))
            .filter(|(_, _, r)| r.addr.is_some_and(|a| a.ssi == ISSI))
            .map(|(c, t, r)| (c, t, r.chan_alloc_element.is_some()))
            .collect();
        assert_eq!(mine.first(), Some(&(MAIN_CARRIER, 1, !goes)), "channel gone: {goes}, {mine:?}");
    }
}

/// A PDU queued for the radio on the MCCH when its carrier assignment goes out follows it to its
/// channel. (miura: here the assignment fills its block, so the PDU behind it cannot share it;
/// tea2 used an encrypted PDU, which must open its block.)
#[test]
fn the_mcch_items_follow_the_radio_to_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = UmacAir::new(carrier_config(false, 4, false));
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], false);
    air.tick();
    let req = |category: Option<i32>, chan_alloc: Option<CmceChanAllocReq>, bits: &str| SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr(bits),
            main_address: TetraAddress::issi(ISSI),
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: category,
            carrier_num: Some(SECONDARY_CARRIER),
            chan_alloc,
            tx_reporter: None,
        }),
    };
    air.test.submit_message(req(
        Some(DATA_CATEGORY_PDCH_ASSIGNMENT),
        Some(CmceChanAllocReq {
            usage: None,
            carrier: Some(SECONDARY_CARRIER),
            timeslots: [true; 4],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        &"0000100110011000".repeat(12),
    ));
    air.test.submit_message(req(None, None, &"0010".repeat(12)));
    for _ in 0..24 {
        air.tick();
    }
    let mine: Vec<(u16, u8, bool)> = air
        .slots
        .iter()
        .flat_map(|s| mac_resources(s).into_iter().map(move |r| (s.carrier_num, s.ts.t, r)))
        .filter(|(_, _, r)| r.addr.is_some_and(|a| a.ssi == ISSI))
        .map(|(c, t, r)| (c, t, r.chan_alloc_element.is_some()))
        .collect();
    assert_eq!(mine.first(), Some(&(MAIN_CARRIER, 1, true)), "the assignment on the MCCH: {mine:?}");
    assert!(
        mine.len() >= 2 && mine[1..].iter().all(|m| m.0 == SECONDARY_CARRIER),
        "the rest on its channel: {mine:?}"
    );
}

/// The advanced link of a radio on four carrier slots: its segments go on all four, in S(S)
/// order, whole, in well under half the time of one slot; the reply slot of a segment asking for
/// an acknowledgement is four opportunities away at least, never on ts1.
#[test]
fn al_segments_spread_over_four_carrier_slots() {
    debug::setup_logging_verbose();
    let (_, one_done) = al_transfer_on(pdch_config(false), &[4], 300);
    let mut mac = MacAir::with(carrier_config(false, 4, false));
    grant_carrier_slots(&mac.test.config, ISSI, &[4, 3, 2, 1], true);
    let reporter = mac.transfer(300);
    let mut segments = Vec::new();
    let mut done = None;
    for tick in 0..4 * 40 {
        mac.test.run_stack(Some(1));
        for msg in mac.test.dump_sinks() {
            let SapMsgInner::TmvUnitdataReqSlots(slots) = msg.msg else {
                continue;
            };
            for slot in slots.slots.iter().filter(|s| s.carrier_num == SECONDARY_CARRIER) {
                for (header, grant) in al_data_in(slot, ISSI) {
                    segments.push((slot.ts, header, grant));
                }
            }
        }
        if done.is_none() && reporter.get_state() == TxState::Transmitted {
            done = Some(tick);
        }
    }
    let done = done.expect("the TL-SDU went out");
    let first: Vec<u8> = segments.iter().take(13).map(|s| s.1.ss).collect();
    assert_eq!(first, (0..13).collect::<Vec<u8>>(), "in S(S) order");
    for ts in 1..=4 {
        assert!(segments.iter().any(|s| s.0.t == ts), "on ts{ts}: {segments:?}");
    }
    assert!(done * 2 <= one_done, "four slots {done} ticks, one slot {one_done} ticks");
    for (t, header, grant) in &segments {
        if let Some((n, delay)) = grant {
            assert!(header.acknowledgement_requested && *n == 1 && *delay >= 4, "{t}: {grant:?}");
            let label = ms_granted_labels(*t, *delay, 1, &[1, 2, 3, 4]);
            assert_ne!(label[0].t, 1, "{t}: {label:?}");
        }
    }
}

/// The adjacent-channel copy of an uplink is dropped: the same bits on the main carrier after
/// the real burst on the carrier (the PHY hands the stronger over first); a lone copy on a main
/// slot where the radio has neither call nor channel; the reverse (the radio back on the MCCH).
/// A copy handed over first (another burst on its carrier was stronger) and dropped there does
/// not drop the real burst after it. Without a carrier channel nothing is dropped.
#[test]
fn a_copy_of_a_carrier_uplink_is_dropped() {
    debug::setup_logging_verbose();
    let setup = |grant: bool| {
        let mut air = UmacAir::new(carrier_config(false, 4, false));
        if grant {
            grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
        }
        air.tick();
        air
    };
    // Both received in the same uplink slot `ts`, `first` handed over first.
    let both = |ts: u8, first: (u16, f32), second: (u16, f32)| {
        let mut air = setup(true);
        let label = air.next_time().add_timeslots(4).forward_to_timeslot(ts);
        let at = label.add_timeslots(2);
        while air.next_time() != at {
            air.tick();
        }
        air.test.submit_message(from_lmac_on(mac_data_of(ISSI), first.0, first.1));
        air.test.submit_message(from_lmac_on(mac_data_of(ISSI), second.0, second.1));
        air.tick();
        air.up.iter().map(|u| u.carrier_num).collect::<Vec<_>>()
    };
    assert_eq!(both(3, (SECONDARY_CARRIER, -40.0), (MAIN_CARRIER, -75.0)), vec![SECONDARY_CARRIER]);
    assert_eq!(
        both(1, (MAIN_CARRIER, -40.0), (SECONDARY_CARRIER, -75.0)),
        vec![MAIN_CARRIER],
        "back on the MCCH"
    );
    assert_eq!(
        both(3, (MAIN_CARRIER, -60.0), (SECONDARY_CARRIER, -40.0)),
        vec![SECONDARY_CARRIER],
        "the copy first"
    );
    for grant in [true, false] {
        let mut air = setup(grant);
        let label = air.next_time().add_timeslots(4).forward_to_timeslot(2);
        air.uplink_at(label, from_lmac_on(mac_data_of(ISSI), MAIN_CARRIER, -75.0));
        air.tick();
        assert_eq!(air.up.len(), usize::from(!grant), "a lone main ts2 burst, carrier channel {grant}");
    }
}

/// `from_lmac_on` as the LMAC hands over a burst of logical channel `lchan` in PHY block `block`.
fn from_lmac_as(
    pdu: BitBuffer,
    carrier: u16,
    rssi_dbfs: f32,
    lchan: tetra_saps::tmv::enums::logical_chans::LogicalChannel,
    block: tetra_core::PhyBlockNum,
) -> SapMsg {
    let mut msg = from_lmac_on(pdu, carrier, rssi_dbfs);
    if let SapMsgInner::TmvUnitdataInd(ind) = &mut msg.msg {
        ind.logical_channel = lchan;
        ind.block_num = block;
    }
    msg
}

/// `mac_data_of` in a half slot (STCH, 124 bits).
fn stch_mac_data_of(issi: u32) -> BitBuffer {
    let mut buf = BitBuffer::from_bitstr(&mac_data_of(issi).to_bitstr()[..124]);
    buf.seek(0);
    buf
}

/// A MAC-ACCESS of `issi` in the clear filling an SCH/HU block (92 bits), a TM-SDU of 24 bits and
/// fill bits, asking for `req` when given.
fn mac_access_of(issi: u32, req: Option<tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement>) -> BitBuffer {
    let optional = req.map_or("0".to_string(), |req| format!("110{:04b}", req as u64));
    let mut b = format!("01000{:024b}{}{}1", issi, optional, "1011".repeat(6));
    while b.len() < 92 {
        b.push('0');
    }
    let mut buf = BitBuffer::from_bitstr(&b);
    buf.seek(0);
    buf
}

/// A group call opened on `ts` of `carrier`: in hangtime, or with `talker` talking.
fn call_on(air: &mut UmacAir, carrier: u16, ts: u8, talker: Option<u32>) {
    use tetra_core::Direction;
    use tetra_saps::control::call_control::{CallControl, Circuit, CircuitDlMediaSource};
    use tetra_saps::control::enums::circuit_mode_type::CircuitModeType;
    let control = |ctrl: CallControl| SapMsg {
        sap: Sap::Control,
        src: TetraEntity::Cmce,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::CmceCallControl(ctrl),
    };
    air.test.submit_message(control(CallControl::Open(Circuit {
        direction: Direction::Both,
        carrier_num: carrier,
        ts,
        peer_carrier_num: None,
        peer_ts: None,
        usage: 6,
        circuit_mode: CircuitModeType::TchS,
        speech_service: Some(0),
        etee_encrypted: false,
        dl_media_source: CircuitDlMediaSource::SwMI,
    })));
    air.test.submit_message(control(match talker {
        Some(source_issi) => CallControl::FloorGranted {
            call_id: 1,
            source_issi,
            dest_gssi: 91,
            carrier_num: carrier,
            ts,
        },
        None => CallControl::FloorReleased {
            call_id: 1,
            carrier_num: carrier,
            ts,
        },
    }));
}

/// The radio of the carrier channel `channel` asks there for `req` on ts3 and is granted uplink
/// slots of it, then joins a group call on main ts3 (in hangtime, or `talking`) while its channel
/// is still there. Returns the UMAC and the carrier ts3 slots the radio holds.
fn in_a_main_call_holding_carrier_slots(
    mut air: UmacAir,
    channel: &[u8],
    req: tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement,
    talking: bool,
) -> (UmacAir, Vec<TdmaTime>) {
    grant_carrier_slots(&air.test.config, ISSI, channel, true);
    let start = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let header = format!("001000{:024b}10{:04b}0", ISSI, req as u64);
    air.uplink_at(
        start,
        from_lmac_on(uplink_block(&header, &"1100".repeat(20)), SECONDARY_CARRIER, -40.0),
    );
    let (at, slots, delay) = air.next_grant(ISSI, 16);
    let held: Vec<TdmaTime> = ms_granted_labels(at, delay, slots, channel)
        .into_iter()
        .filter(|l| l.t == 3)
        .collect();
    assert!(!held.is_empty(), "a carrier ts3 slot granted");
    call_on(&mut air, MAIN_CARRIER, 3, talking.then_some(ISSI));
    (air, held)
}

/// `msgs` received in uplink slot `u`, handed over in this order: the carriers of what reached
/// the LLC.
fn heard_at(air: &mut UmacAir, u: TdmaTime, msgs: Vec<SapMsg>) -> Vec<u16> {
    let at = u.add_timeslots(2);
    while air.next_time() != at {
        assert!(air.next_time().diff(at) < 0, "uplink slot {u} is past");
        air.tick();
    }
    air.up.clear();
    for msg in msgs {
        air.test.submit_message(msg);
    }
    air.tick();
    air.up.iter().map(|u| u.carrier_num).collect()
}

/// A radio moved from its carrier channel to a call on main ts3 while it still holds uplink slots
/// of its channel there (granted capacity is not withdrawn, 23.5.2.2.2): its random access and
/// its stolen block on main ts3 in one of those slots are its own (it sends only SCH/F in a
/// granted slot, 23.5.2.3.1), taken, and their copies on the carrier dropped. A lone SCH/F there
/// is still the copy of its carrier grant.
#[test]
fn a_radio_in_a_main_call_is_heard_in_the_slots_it_still_holds_on_the_carrier() {
    use tetra_core::PhyBlockNum;
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    // In the first carrier ts3 slot it holds, with main ts3 in hangtime or the radio talking there.
    let heard = |talking: bool, msgs: &dyn Fn() -> Vec<SapMsg>| {
        let air = UmacAir::new(carrier_config(false, 4, false));
        let (mut air, held) = in_a_main_call_holding_carrier_slots(air, &[4, 3, 2], ReservationRequirement::Req4Slots, talking);
        heard_at(&mut air, held[0], msgs())
    };
    let access = |carrier, rssi| from_lmac_as(mac_access_of(ISSI, None), carrier, rssi, LogicalChannel::SchHu, PhyBlockNum::Block1);
    let stolen = |carrier, rssi| from_lmac_as(stch_mac_data_of(ISSI), carrier, rssi, LogicalChannel::Stch, PhyBlockNum::Block1);
    assert_eq!(
        [
            heard(false, &|| vec![access(MAIN_CARRIER, -40.0), access(SECONDARY_CARRIER, -75.0)]),
            heard(true, &|| vec![stolen(MAIN_CARRIER, -40.0), stolen(SECONDARY_CARRIER, -75.0)]),
            heard(false, &|| vec![from_lmac_on(mac_data_of(ISSI), MAIN_CARRIER, -40.0)]),
        ],
        [vec![MAIN_CARRIER], vec![MAIN_CARRIER], vec![]],
        "SCH/HU in hangtime, STCH while it talks, a lone SCH/F"
    );
}

/// A radio in hangtime of a call on main ts3, granted uplink slots there, then granted uplink
/// slots of its carrier channel too: in a slot it holds on both, the SCH/F it sends on main ts3
/// in its grant there is taken (one of the two is stale, the burst handed over first is the
/// real one) and its copy on the carrier dropped.
#[test]
fn a_radio_in_a_main_call_uses_its_main_grant_in_a_slot_it_still_holds_on_the_carrier() {
    use tetra_core::PhyBlockNum;
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let mut air = UmacAir::new(carrier_config(false, 4, false));
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2], true);
    call_on(&mut air, MAIN_CARRIER, 3, None);
    // Grants of the radio on `carrier` from slot `from` on, read as the MS does on `channel`.
    let granted = |air: &UmacAir, from: usize, carrier: u16, channel: &[u8]| -> Vec<TdmaTime> {
        air.slots[from..]
            .iter()
            .filter(|s| s.carrier_num == carrier)
            .filter_map(|s| grant_for(s, ISSI).map(|(n, delay)| ms_granted_labels(s.ts, delay, n, channel)))
            .flatten()
            .collect()
    };

    let first = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let access = mac_access_of(ISSI, Some(ReservationRequirement::Req8Slots));
    let from = air.slots.len();
    air.uplink_at(
        first,
        from_lmac_as(access, MAIN_CARRIER, -40.0, LogicalChannel::SchHu, PhyBlockNum::Block1),
    );
    for _ in 0..16 {
        air.tick();
    }
    let on_main = granted(&air, from, MAIN_CARRIER, &[3]);
    assert!(!on_main.is_empty(), "granted on main ts3");

    let request = air.next_time().forward_to_timeslot(4);
    let header = format!("001000{:024b}10{:04b}0", ISSI, ReservationRequirement::Req8Slots as u64);
    let from = air.slots.len();
    air.uplink_at(
        request,
        from_lmac_on(uplink_block(&header, &"1100".repeat(20)), SECONDARY_CARRIER, -40.0),
    );
    let v = (0..32)
        .find_map(|_| {
            air.tick();
            let now = air.next_time();
            granted(&air, from, SECONDARY_CARRIER, &[2, 3, 4])
                .into_iter()
                .find(|l| l.t == 3 && on_main.contains(l) && l.add_timeslots(2).diff(now) >= 0)
        })
        .expect("a ts3 slot held on both carriers");

    let data = |carrier, rssi| from_lmac_on(mac_data_of(ISSI), carrier, rssi);
    assert_eq!(
        heard_at(&mut air, v, vec![data(MAIN_CARRIER, -40.0), data(SECONDARY_CARRIER, -75.0)]),
        vec![MAIN_CARRIER]
    );
}

/// The copy on main ts3 of a burst the radio sends in a carrier slot it holds, handed over first
/// (another burst on the main carrier was stronger), is still dropped, and the real burst after
/// it taken: in a full slot, and in a subslot of a channel of one slot.
#[test]
fn rule_two_still_drops_a_copy_handed_over_first() {
    use tetra_core::PhyBlockNum;
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let air = UmacAir::new(carrier_config(false, 4, false));
    let (mut air, held) = in_a_main_call_holding_carrier_slots(air, &[4, 3, 2], ReservationRequirement::Req4Slots, true);
    let data = |carrier, rssi| from_lmac_on(mac_data_of(ISSI), carrier, rssi);
    assert_eq!(
        heard_at(&mut air, held[0], vec![data(MAIN_CARRIER, -60.0), data(SECONDARY_CARRIER, -40.0)]),
        vec![SECONDARY_CARRIER],
        "a slot"
    );

    let air = UmacAir::new(carrier_config(false, 4, false));
    let (mut air, held) = in_a_main_call_holding_carrier_slots(air, &[3], ReservationRequirement::Req1Subslot, true);
    let access = |carrier, rssi| from_lmac_as(mac_access_of(ISSI, None), carrier, rssi, LogicalChannel::SchHu, PhyBlockNum::Block1);
    assert_eq!(
        heard_at(
            &mut air,
            held[0],
            vec![access(MAIN_CARRIER, -60.0), access(SECONDARY_CARRIER, -40.0)]
        ),
        vec![SECONDARY_CARRIER],
        "a subslot"
    );
}

/// The radio in hangtime of a call on main ts3 while it still holds uplink slots of its carrier
/// channel [3, 2] on ts3 (ISSI2's channel on ts4 stays on the carrier), then its channel released
/// and a call opened on carrier ts3 with `talker` talking. Returns the UMAC and the last carrier
/// ts3 slot the radio holds.
fn in_a_main_call_with_its_carrier_slot_in_a_call(talker: u32) -> (UmacAir, TdmaTime) {
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    let air = UmacAir::new(carrier_config(false, 4, false));
    grant_carrier_slots(&air.test.config, ISSI2, &[4], true);
    let (mut air, held) = in_a_main_call_holding_carrier_slots(air, &[3, 2], ReservationRequirement::Req4Slots, false);
    {
        let mut state = air.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for ts in [3, 2] {
            state
                .timeslot_alloc
                .release_slot(TimeslotOwner::PacketData, carrier_slot(ts))
                .unwrap();
        }
    }
    air.tick();
    call_on(&mut air, SECONDARY_CARRIER, 3, Some(talker));
    let u = *held.last().unwrap();
    (air, u)
}

/// A call takes the radio's carrier ts3 after its channel is released (another radio's channel
/// stays on the carrier), and the radio joins it there while it still holds uplink slots of its
/// old channel on ts3: the copy on main ts3 (in a call too) of its stolen block there, handed
/// over first, is dropped; the block is taken.
#[test]
fn a_copy_is_dropped_while_a_call_holds_the_carrier_slot() {
    use tetra_core::PhyBlockNum;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let (mut air, u) = in_a_main_call_with_its_carrier_slot_in_a_call(ISSI);
    let stolen = |carrier, rssi| from_lmac_as(stch_mac_data_of(ISSI), carrier, rssi, LogicalChannel::Stch, PhyBlockNum::Block1);
    assert_eq!(
        heard_at(&mut air, u, vec![stolen(MAIN_CARRIER, -60.0), stolen(SECONDARY_CARRIER, -40.0)]),
        vec![SECONDARY_CARRIER]
    );
}

/// As above, but the call on carrier ts3 is another group's, ISSI2 talking: that uplink is closed
/// to the radio (traffic usage marker, 23.5.5), so its random access on main ts3 (in hangtime) in
/// a slot it still holds on carrier ts3 is its own, taken, and its copy on the carrier dropped.
#[test]
fn a_radio_is_heard_on_main_while_another_radio_talks_on_its_old_carrier_slot() {
    use tetra_core::PhyBlockNum;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let (mut air, u) = in_a_main_call_with_its_carrier_slot_in_a_call(ISSI2);
    let access = |carrier, rssi| from_lmac_as(mac_access_of(ISSI, None), carrier, rssi, LogicalChannel::SchHu, PhyBlockNum::Block1);
    assert_eq!(
        heard_at(&mut air, u, vec![access(MAIN_CARRIER, -40.0), access(SECONDARY_CARRIER, -75.0)]),
        vec![MAIN_CARRIER]
    );
}

/// The radio's channel, the last one of the carrier, released while it is in hangtime of a call
/// on main ts3: for a multiframe its bursts on the carrier ts3 it left still pass as its own
/// (23.5.2.2.4 NOTE 2), so the copy rules keep running: its random access on main ts3 is taken
/// once, the copy on the carrier dropped.
#[test]
fn a_copy_is_dropped_for_a_multiframe_after_the_last_carrier_channel_went() {
    use tetra_core::PhyBlockNum;
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let air = UmacAir::new(carrier_config(false, 4, false));
    let (mut air, held) = in_a_main_call_holding_carrier_slots(air, &[3, 2], ReservationRequirement::Req4Slots, false);
    {
        let mut state = air.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for ts in [3, 2] {
            state
                .timeslot_alloc
                .release_slot(TimeslotOwner::PacketData, carrier_slot(ts))
                .unwrap();
        }
    }
    air.tick();
    let u = *held.last().unwrap();
    let access = |carrier, rssi| from_lmac_as(mac_access_of(ISSI, None), carrier, rssi, LogicalChannel::SchHu, PhyBlockNum::Block1);
    assert_eq!(
        heard_at(&mut air, u, vec![access(MAIN_CARRIER, -40.0), access(SECONDARY_CARRIER, -75.0)]),
        vec![MAIN_CARRIER]
    );
}

// Three carriers (`extra_carriers`): the copy rules always run, against any other carrier --------

const THIRD_CARRIER: u16 = 1524;

/// Packet data off, the secondary carrier and the extra carriers `extra`.
fn cell_of(extra: &[u16]) -> StackConfig {
    let mut cfg = voice_config(None, true);
    cfg.cell.extra_carriers = extra.to_vec();
    cfg
}

/// The next uplink ts2 slot at least a frame ahead.
fn next_ts2(air: &UmacAir) -> TdmaTime {
    air.next_time().add_timeslots(4).forward_to_timeslot(2)
}

/// Dual carrier without a packet-data carrier runs no copy rule, as on 8092d84: a lone MAC-DATA on
/// secondary ts2 or main ts2, where no call is, reaches the LLC.
#[test]
fn dual_carrier_without_a_packet_data_carrier_drops_nothing() {
    debug::setup_logging_verbose();
    let cfg = cell_of(&[]);
    assert!(!cfg.uplink_copy_rules(), "nor does the PHY order the carriers by RSSI");
    let mut air = UmacAir::new(cfg);
    air.tick();
    for carrier in [SECONDARY_CARRIER, MAIN_CARRIER] {
        let u = next_ts2(&air);
        assert_eq!(
            heard_at(&mut air, u, vec![from_lmac_on(mac_data_of(ISSI), carrier, -40.0)]),
            vec![carrier]
        );
    }
}

/// Rule 1 on a third carrier without a packet-data carrier: a lone MAC-DATA on third ts2 is a copy
/// without a call there, taken with one (in hangtime).
#[test]
fn three_carriers_rule_one_on_the_third() {
    debug::setup_logging_verbose();
    let lone = |call: bool| {
        let mut air = UmacAir::new(cell_of(&[THIRD_CARRIER]));
        if call {
            call_on(&mut air, THIRD_CARRIER, 2, None);
        }
        air.tick();
        let u = next_ts2(&air);
        heard_at(&mut air, u, vec![from_lmac_on(mac_data_of(ISSI), THIRD_CARRIER, -40.0)])
    };
    assert_eq!(lone(false), Vec::<u16>::new());
    assert_eq!(lone(true), vec![THIRD_CARRIER]);
}

/// Rule 2 against any other carrier: the radio in hangtime of a call on secondary ts3 is granted
/// uplink slots there; in one of them a lone burst of it on third ts3 (a call there too) is the
/// copy of what it sends on the secondary, dropped. On the secondary it is taken, and on third ts3
/// in a slot it does not hold, too.
#[test]
fn three_carriers_rule_two_against_any_other_carrier() {
    use tetra_core::PhyBlockNum;
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let granted = || {
        let mut air = UmacAir::new(cell_of(&[THIRD_CARRIER]));
        call_on(&mut air, SECONDARY_CARRIER, 3, None);
        call_on(&mut air, THIRD_CARRIER, 3, None);
        let access = mac_access_of(ISSI, Some(ReservationRequirement::Req4Slots));
        air.uplink_at(
            TdmaTime { t: 3, f: 3, m: 1, h: 0 },
            from_lmac_as(access, SECONDARY_CARRIER, -40.0, LogicalChannel::SchHu, PhyBlockNum::Block1),
        );
        let from = air.slots.len();
        let (at, slots, delay) = air.next_grant(ISSI, 16);
        assert!(
            air.slots[from..]
                .iter()
                .any(|s| s.ts == at && s.carrier_num == SECONDARY_CARRIER && grant_for(s, ISSI).is_some()),
            "granted on the secondary"
        );
        (air, ms_granted_labels(at, delay, slots, &[3]))
    };
    let data = |carrier| from_lmac_on(mac_data_of(ISSI), carrier, -40.0);
    let (mut air, held) = granted();
    assert_eq!(heard_at(&mut air, held[0], vec![data(THIRD_CARRIER)]), Vec::<u16>::new());
    let (mut air, held) = granted();
    assert_eq!(heard_at(&mut air, held[0], vec![data(SECONDARY_CARRIER)]), vec![SECONDARY_CARRIER]);
    let (mut air, held) = granted();
    let mut later = held.last().unwrap().add_timeslots(4 * 8);
    if later.f == 18 {
        later = later.add_timeslots(4);
    }
    assert_eq!(heard_at(&mut air, later, vec![data(THIRD_CARRIER)]), vec![THIRD_CARRIER]);
}

/// Rule 3 without a packet-data carrier: a lone MAC-DATA on main ts2 without a call there is a
/// copy, dropped; the first stolen block of the radio a call there was just opened for is taken.
#[test]
fn three_carriers_rule_three_on_main() {
    use tetra_core::PhyBlockNum;
    use tetra_saps::tmv::enums::logical_chans::LogicalChannel;
    debug::setup_logging_verbose();
    let mut air = UmacAir::new(cell_of(&[THIRD_CARRIER]));
    air.tick();
    let u = next_ts2(&air);
    assert_eq!(
        heard_at(&mut air, u, vec![from_lmac_on(mac_data_of(ISSI), MAIN_CARRIER, -40.0)]),
        Vec::<u16>::new()
    );
    call_on(&mut air, MAIN_CARRIER, 2, Some(ISSI));
    air.tick();
    let u = air.next_time().forward_to_timeslot(2);
    let stolen = from_lmac_as(stch_mac_data_of(ISSI), MAIN_CARRIER, -40.0, LogicalChannel::Stch, PhyBlockNum::Block1);
    assert_eq!(heard_at(&mut air, u, vec![stolen]), vec![MAIN_CARRIER]);
}

/// The same block received on two or three carriers, each with a call in that slot, is taken
/// once, from the carrier handed over first.
#[test]
fn three_carriers_take_a_block_once() {
    debug::setup_logging_verbose();
    let heard = |order: &[u16]| {
        let mut air = UmacAir::new(cell_of(&[THIRD_CARRIER]));
        for carrier in [MAIN_CARRIER, SECONDARY_CARRIER, THIRD_CARRIER] {
            call_on(&mut air, carrier, 2, None);
        }
        air.tick();
        let u = next_ts2(&air);
        heard_at(
            &mut air,
            u,
            order.iter().map(|c| from_lmac_on(mac_data_of(ISSI), *c, -40.0)).collect(),
        )
    };
    assert_eq!(heard(&[SECONDARY_CARRIER, THIRD_CARRIER]), vec![SECONDARY_CARRIER]);
    assert_eq!(
        heard(&[THIRD_CARRIER, MAIN_CARRIER, SECONDARY_CARRIER]),
        vec![THIRD_CARRIER]
    );
}

/// The packet-data carrier on the third carrier: a radio that can take 4 slots is sent there, and
/// voice fills the main carrier and the secondary first; then, shared, it takes a data slot, data
/// only never.
#[test]
fn a_packet_data_carrier_on_the_third_carrier() {
    debug::setup_logging_verbose();
    for exclusive in [false, true] {
        let mut cfg = carrier_config(false, 4, exclusive);
        cfg.cell.extra_carriers = vec![THIRD_CARRIER];
        cfg.packet_data.pdch_carrier = Some(THIRD_CARRIER);
        let mut air = Air::new(cfg);
        assert_eq!(
            onto_carrier(&mut air, ISSI),
            Some((Some(THIRD_CARRIER), vec![1, 2, 3, 4])),
            "exclusive {exclusive}"
        );
        let mut state = air.test.config.state_write();
        let mut voice = || state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).map(|s| s.carrier_num);
        let first: Vec<Option<u16>> = (0..7).map(|_| voice()).collect();
        assert_eq!(first, [[Some(MAIN_CARRIER); 3].as_slice(), &[Some(SECONDARY_CARRIER); 4]].concat());
        assert_eq!(voice(), (!exclusive).then_some(THIRD_CARRIER), "exclusive {exclusive}");
    }
}

/// With the packet-data carrier on the secondary and a third carrier, voice fills the third one
/// before the packet-data carrier.
#[test]
fn with_three_carriers_voice_fills_the_packet_data_carrier_last() {
    let mut cfg = carrier_config(false, 4, false);
    cfg.cell.extra_carriers = vec![THIRD_CARRIER];
    let shared = SharedConfig::from_parts(cfg, None);
    let mut state = shared.state_write();
    let voice: Vec<u16> = (0..11)
        .map(|_| state.timeslot_alloc.allocate_any_slot(TimeslotOwner::Cmce).expect("voice").carrier_num)
        .collect();
    assert_eq!(
        voice,
        [[MAIN_CARRIER; 3].as_slice(), &[THIRD_CARRIER; 4], &[SECONDARY_CARRIER; 4]].concat()
    );
}

/// ISSI2 on its main-carrier channel (ts3) has an uplink fragmentation open and one slot granted
/// for its MAC-END; in that same uplink slot the radio of a carrier channel sends a MAC-END on the
/// carrier, received first, and the main carrier gets either that block's adjacent-channel copy
/// (weaker) or ISSI2's own MAC-END. What reaches the LLC, and ISSI2's TM-SDU.
fn with_an_addressless_copy(own_end: bool) -> (Vec<TmaUnitdataInd>, String) {
    use tetra_pdus::umac::enums::reservation_requirement::ReservationRequirement;
    let mut air = UmacAir::new(carrier_config(false, 4, false));
    grant_pdch(&air.test.config, ISSI2, 3, true);
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2], true);
    let sdu: String = (0..250).map(|i| if (i * 5) % 3 == 0 { '1' } else { '0' }).collect();
    let (first, end) = (&sdu[..150], &sdu[150..]);
    let start = TdmaTime { t: 3, f: 3, m: 1, h: 0 };
    let header = format!("001000{:024b}11{:04b}0", ISSI2, ReservationRequirement::Req1Slot as u64);
    air.uplink_at(start, from_lmac(uplink_block(&header, first)));
    let (at, slots, delay) = air.next_grant(ISSI2, 16);
    assert_eq!(slots, 1);
    let label = ms_granted_labels(at, delay, slots, &[3])[0];
    let end_block = |bits: &str| uplink_block(&format!("0111{:06b}", (10 + bits.len() + 1).div_ceil(8)), bits);
    let other_end = "1100".repeat(25);
    while air.next_time() != label.add_timeslots(2) {
        air.tick();
    }
    air.test
        .submit_message(from_lmac_on(end_block(&other_end), SECONDARY_CARRIER, -40.0));
    let on_main = if own_end {
        from_lmac_on(end_block(end), MAIN_CARRIER, -45.0)
    } else {
        from_lmac_on(end_block(&other_end), MAIN_CARRIER, -75.0)
    };
    air.test.submit_message(on_main);
    for _ in 0..4 {
        air.tick();
    }
    (air.up, sdu)
}

/// An addressless copy (a MAC-END of the carrier) is not appended to the reassembly of the main
/// channel's radio that holds the slot: dropped as a bit copy before attribution; that radio's
/// own, different MAC-END in the same slot is taken.
#[test]
fn an_addressless_copy_is_not_appended_to_another_radios_reassembly() {
    debug::setup_logging_verbose();
    let (up, _) = with_an_addressless_copy(false);
    assert!(up.is_empty(), "{up:?}");
    let (up, sdu) = with_an_addressless_copy(true);
    assert_eq!(up.len(), 1, "{up:?}");
    assert_eq!(
        (up[0].carrier_num, up[0].main_address.ssi, up[0].pdu.as_ref().unwrap().to_bitstr()),
        (MAIN_CARRIER, ISSI2, sdu)
    );
}

/// The carrier channel on the dashboard: its slot active on carrier 1522, its downlink data too.
#[test]
fn the_dashboard_shows_the_carrier_pdch() {
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};
    debug::setup_logging_verbose();
    let (sink, source) = telemetry_channel();
    let mut cfg = voice_config(Some(PacketDataBearer::Pdch), true);
    cfg.packet_data.pdch_carrier = Some(SECONDARY_CARRIER);
    let mut v = Voice::with_telemetry(cfg, true, Some(sink));
    voice_onto_pdch(&mut v);
    let onto = pdch_telemetry(&source);
    let active = TelemetryEvent::PdchChanged {
        carrier_num: SECONDARY_CARRIER,
        ts: 4,
        issi: DATA_RADIO,
        active: true,
    };
    assert_eq!(format!("{onto:?}"), format!("{:?}", [active]));
    v.test.submit_message(tl_unitdata_to(TetraAddress::issi(DATA_RADIO), 16, None));
    v.run(8);
    let traffic = pdch_telemetry(&source);
    assert!(
        traffic.iter().any(|e| matches!(
            e,
            TelemetryEvent::TsDataActivity {
                carrier_num: SECONDARY_CARRIER,
                ts: 4,
                issi: DATA_RADIO,
                uplink: false
            }
        )),
        "{traffic:?}"
    );
}

/// The dashboard sees every slot of a four-slot channel of the packet-data carrier, each on that
/// carrier: PDCH active and inactive on ts 1 to 4, and the downlink to the radio on each (they
/// all carry it). Nothing is reported on the main carrier.
#[test]
fn the_dashboard_shows_every_slot_of_a_carrier_pdch() {
    use tetra_entities::net_telemetry::{TelemetryEvent, telemetry_channel};
    let (sink, source) = telemetry_channel();
    let mut test = ComponentTest::from_config(carrier_config(false, 4, false), Some(TdmaTime::default()));
    test.populate_entities(vec![], vec![TetraEntity::Llc, TetraEntity::Lmac]);
    test.register_entity(tetra_entities::umac::umac_bs::UmacBs::new(test.get_shared_config(), Some(sink)));
    let slots = grant_carrier_slots(&test.config, ISSI, &[4, 3, 2, 1], true);
    test.run_stack(Some(2));
    let changed = |events: &[TelemetryEvent], active: bool| -> Vec<(u16, u8)> {
        let mut on: Vec<(u16, u8)> = events
            .iter()
            .filter_map(|e| match e {
                TelemetryEvent::PdchChanged {
                    carrier_num,
                    ts,
                    issi: ISSI,
                    active: a,
                } if *a == active => Some((*carrier_num, *ts)),
                _ => None,
            })
            .collect();
        on.sort_unstable();
        on
    };
    let all: Vec<(u16, u8)> = (1..=4).map(|ts| (SECONDARY_CARRIER, ts)).collect();
    assert_eq!(changed(&pdch_telemetry(&source), true), all);

    let mut pdu = BitBuffer::new_autoexpand(32);
    BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    append_bits(&mut pdu, "0101010101");
    test.submit_message(SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            carrier_num: Some(SECONDARY_CARRIER),
            req_handle: 0,
            pdu,
            main_address: TetraAddress::issi(ISSI),
            link_id: 4,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: None,
            chan_alloc: None,
            tx_reporter: None,
        }),
    });
    test.run_stack(Some(1));
    let mut down: Vec<(u16, u8)> = pdch_telemetry(&source)
        .iter()
        .filter_map(|e| match e {
            TelemetryEvent::TsDataActivity {
                carrier_num,
                ts,
                issi: ISSI,
                uplink: false,
            } => Some((*carrier_num, *ts)),
            _ => None,
        })
        .collect();
    down.sort_unstable();
    assert_eq!(down, all);

    {
        let mut state = test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for s in slots {
            state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, s).unwrap();
        }
    }
    test.run_stack(Some(1));
    assert_eq!(changed(&pdch_telemetry(&source), false), all);
}

/// The WAP gateway through a channel of the packet-data carrier: the radio is sent to four slots
/// of the carrier, its WSP GET goes up on one of them and the gateway's answer comes down there,
/// on its channel, never stolen.
#[test]
fn the_wap_gateway_answers_on_the_carrier() {
    debug::setup_logging_verbose();
    let mut air = Air::new(carrier_config(true, 4, false));
    air.send(ISSI, &demand(1, None, true));
    let ip = accept_ip(&air.next_sn(8).unwrap());
    air.send(ISSI, &transmit_request_full(1, 4, 4));
    let response = air.next_sn(8).unwrap();
    assert_eq!(
        response.alloc.as_ref().map(|a| (a.carrier, ts_of(a))),
        Some((Some(SECONDARY_CARRIER), vec![1, 2, 3, 4]))
    );
    air.run(2);
    assert!(air.test.config.state_read().pdch_by_issi[&ISSI].on_air);
    air.take_sn();
    let mut pdu = BitBuffer::new_autoexpand(64);
    BlUdata { has_fcs: false }.to_bitbuf(&mut pdu);
    let get = unitdata(1, &datagram(ip, GATEWAY, 9201, &wtp_get(0x51, "/status.wml")));
    append_bits(&mut pdu, &format!("100{get}"));
    air.uplink_on_carrier(ISSI, pdu, 3);
    let answer = air.next_sn(300).expect("the answer");
    assert_eq!(
        (answer.sn_type(), answer.carrier, answer.link_id, answer.stealing),
        (Some(4), Some(SECONDARY_CARRIER), 4, false)
    );
    wtp_down(&answer, ip);
}

/// `pdch_carrier` with the secondary carrier off (the dashboard switch): the main carrier as
/// before.
#[test]
fn no_carrier_without_the_secondary() {
    let mut cfg = carrier_config(false, 4, false);
    cfg.cell.secondary_carrier = None;
    let mut air = Air::new(cfg);
    assert_eq!(
        onto_carrier(&mut air, ISSI),
        Some((Some(MAIN_CARRIER), vec![3, 4])),
        "voice headroom"
    );
}

/// With the packet-data carrier N.264 may be 4: an AL-SETUP proposing 4 slots is answered 4.
#[test]
fn al_setup_answers_four_with_the_carrier() {
    let mut setup = radio_al_setup();
    setup.connection_width = true;
    setup.uplink_timeslots = Some(3);
    let mut air = Air::new(carrier_config(false, 4, false));
    let a = air.al_setup_with(ISSI, setup);
    assert_eq!(
        (a.setup_report, a.uplink_timeslots.map(|s| s + 1)),
        (AlSetup::SETUP_REPORT_SUCCESS, Some(4))
    );
}

/// With `pdch_carrier` set and no data session, two duplex calls on two carriers are what they
/// were (the fingerprint recorded before the packet-data carrier).
#[test]
fn dual_identity_duplex_calls_with_a_packet_data_carrier() {
    fn with_carrier(cfg: &mut StackConfig) {
        cfg.packet_data.pdch_carrier = cfg.cell.secondary_carrier;
    }
    let hash = common::component_test::with_config_tweak(with_carrier, || {
        fingerprint(duplex_calls_on_two_carriers_are_the_same_with_packet_data_on)
    });
    assert_eq!(hash, 0xdb61_bf54_cae9_5f0b);
}

// ---------------------------------------------------------------------------------------------
// Carriers on demand (`secondary_carrier_on_demand`) with packet data and through the whole stack
// ---------------------------------------------------------------------------------------------

/// The same with `secondary_carrier_on_demand = false`, whatever the warm-up and grace say.
#[test]
fn dual_identity_with_carriers_on_demand_off() {
    fn on_demand_off(cfg: &mut StackConfig) {
        cfg.packet_data.pdch_carrier = cfg.cell.secondary_carrier;
        cfg.cell.secondary_carrier_on_demand = false;
        cfg.cell.secondary_carrier_warmup_frames = 9;
        cfg.cell.secondary_carrier_inactivity_secs = 1;
    }
    let hash = common::component_test::with_config_tweak(on_demand_off, || {
        fingerprint(duplex_calls_on_two_carriers_are_the_same_with_packet_data_on)
    });
    assert_eq!(hash, 0xdb61_bf54_cae9_5f0b);
}

/// Every carrier but the main one on demand.
fn on_demand(cfg: &mut StackConfig, warmup_frames: u8, inactivity_secs: u16) {
    cfg.cell.secondary_carrier_on_demand = true;
    cfg.cell.secondary_carrier_warmup_frames = warmup_frames;
    cfg.cell.secondary_carrier_inactivity_secs = inactivity_secs;
}

/// The packet-data carrier on demand: the radio's channel there puts it on the air, its
/// assignment goes on the MCCH once the carrier has been on the air for the warm-up, and what was
/// queued for the radio behind it does not overtake it and follows it to its channel.
#[test]
fn the_mcch_items_follow_the_radio_to_a_carrier_on_demand() {
    debug::setup_logging_verbose();
    let mut cfg = carrier_config(false, 4, false);
    on_demand(&mut cfg, 4, 180);
    let mut air = UmacAir::new(cfg);
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], false);
    air.tick();
    let req = |category: Option<i32>, chan_alloc: Option<CmceChanAllocReq>, bits: &str| SapMsg {
        sap: Sap::TmaSap,
        src: TetraEntity::Llc,
        dest: TetraEntity::Umac,
        msg: SapMsgInner::TmaUnitdataReq(TmaUnitdataReq {
            req_handle: 0,
            pdu: BitBuffer::from_bitstr(bits),
            main_address: TetraAddress::issi(ISSI),
            link_id: 0,
            endpoint_id: 0,
            stealing_permission: false,
            subscriber_class: 0,
            air_interface_encryption: None,
            stealing_repeats_flag: None,
            data_category: category,
            carrier_num: Some(SECONDARY_CARRIER),
            chan_alloc,
            tx_reporter: None,
        }),
    };
    air.test.submit_message(req(
        Some(DATA_CATEGORY_PDCH_ASSIGNMENT),
        Some(CmceChanAllocReq {
            usage: None,
            carrier: Some(SECONDARY_CARRIER),
            timeslots: [true; 4],
            alloc_type: ChanAllocType::Replace,
            ul_dl_assigned: UlDlAssignment::Both,
        }),
        &"0000100110011000".repeat(12),
    ));
    air.test.submit_message(req(None, None, &"0010".repeat(12)));
    for _ in 0..48 {
        air.tick();
    }
    let t_on = air
        .slots
        .iter()
        .find(|s| s.carrier_num == SECONDARY_CARRIER)
        .map(|s| s.ts)
        .expect("the carrier went on the air");
    assert_eq!(t_on, air.slots[0].ts, "on the air with the channel");
    let mine: Vec<(u16, u8, bool, TdmaTime)> = air
        .slots
        .iter()
        .flat_map(|s| mac_resources(s).into_iter().map(move |r| (s.carrier_num, s.ts.t, r, s.ts)))
        .filter(|(_, _, r, _)| r.addr.is_some_and(|a| a.ssi == ISSI))
        .map(|(c, t, r, at)| (c, t, r.chan_alloc_element.is_some(), at))
        .collect();
    let first = mine.first().expect("the assignment");
    assert_eq!(
        (first.0, first.1, first.2),
        (MAIN_CARRIER, 1, true),
        "the assignment on the MCCH: {mine:?}"
    );
    assert!(first.3.diff(t_on) >= 16, "after the warm-up: on {t_on}, assignment {}", first.3);
    assert!(
        mine.len() >= 2 && mine[1..].iter().all(|m| m.0 == SECONDARY_CARRIER),
        "the rest on its channel: {mine:?}"
    );
}

/// When the channel goes, the carrier stays on the air for the multiframe its slots show
/// unallocated, then for the grace, and goes off.
#[test]
fn a_carrier_on_demand_stays_on_for_the_released_pdch_and_the_grace() {
    debug::setup_logging_verbose();
    let mut cfg = carrier_config(false, 4, false);
    on_demand(&mut cfg, 4, 1);
    let mut air = UmacAir::new(cfg);
    for _ in 0..8 {
        air.tick();
    }
    assert!(air.slots.iter().all(|s| s.carrier_num == MAIN_CARRIER), "off without a channel");
    let slots = grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    for _ in 0..40 {
        air.tick();
    }
    assert_eq!(air.slots.last().map(|s| s.carrier_num), Some(SECONDARY_CARRIER));
    {
        let mut state = air.test.config.state_write();
        state.pdch_by_issi.remove(&ISSI);
        state.pdch_timeslots_by_issi.remove(&ISSI);
        for s in slots {
            state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, s).unwrap();
        }
    }
    let from = air.slots.len();
    for _ in 0..300 {
        air.tick();
    }
    let on: Vec<TdmaTime> = air.slots[from..]
        .iter()
        .filter(|s| s.carrier_num == SECONDARY_CARRIER)
        .map(|s| s.ts)
        .collect();
    // 73 slots with the released channel (both ends of its multiframe), then 72 of grace.
    assert_eq!(on.len(), 73 + 72, "{:?}..{:?}", on.first(), on.last());
    assert_eq!(on.last().unwrap().diff(on[0]), 73 + 72 - 1, "without a gap");
}

/// A data-only carrier on demand: a full main carrier does not put it on the air (no call goes
/// there), a packet-data channel does.
#[test]
fn a_data_only_carrier_on_demand_goes_on_for_packet_data_only() {
    debug::setup_logging_verbose();
    let mut cfg = carrier_config(false, 4, true);
    on_demand(&mut cfg, 4, 180);
    let mut air = UmacAir::new(cfg);
    {
        let mut state = air.test.config.state_write();
        for ts in 2..=4 {
            state.timeslot_alloc.reserve(TimeslotOwner::Cmce, ts).unwrap();
        }
    }
    for _ in 0..40 {
        air.tick();
    }
    assert!(air.slots.iter().all(|s| s.carrier_num == MAIN_CARRIER), "a full main carrier");
    grant_carrier_slots(&air.test.config, ISSI, &[4, 3, 2, 1], true);
    let from = air.slots.len();
    air.tick();
    assert!(
        air.slots[from..].iter().any(|s| s.carrier_num == SECONDARY_CARRIER),
        "a channel there"
    );
}

/// A group call that overflows the main carrier through the whole stack, the secondary carrier
/// on demand (or always on): the main carrier filled up 20 slots before the U-SETUP (`warm`: the
/// secondary already on the air for the call that would go there) or in the tick of the U-SETUP.
/// What went down from the U-SETUP on.
fn overflowing_group_call(carriers_on_demand: bool, warm: bool) -> Voice {
    let mut cfg = voice_config(None, true);
    if carriers_on_demand {
        on_demand(&mut cfg, 4, 1);
    }
    let mut v = Voice::new(cfg, true);
    v.register(ISSI, Some(GSSI));
    v.register(ISSI2, Some(GSSI));
    let fill_main = |v: &Voice| {
        let mut state = v.test.config.state_write();
        for ts in 2..=4 {
            state.timeslot_alloc.reserve(TimeslotOwner::Brew, ts).unwrap();
        }
    };
    if warm {
        fill_main(&v);
        v.run(20);
    }
    while v.ticks % 4 != 2 {
        v.run(1);
    }
    if !warm {
        fill_main(&v);
    }
    v.msgs.clear();
    v.cmce(ISSI, u_setup(GSSI, true, false, 0));
    v.run(60);
    v
}

/// The slots the real MAC sent down.
fn voice_slots(v: &Voice) -> Vec<tetra_saps::tmv::TmvUnitdataReqSlot> {
    v.msgs
        .iter()
        .filter_map(|m| match &m.msg {
            SapMsgInner::TmvUnitdataReqSlots(slots) => Some(slots.slots.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The MAC-RESOURCEs on the MCCH, sorted: (slot time, SSI, channel allocation).
fn mcch_resources(slots: &[tetra_saps::tmv::TmvUnitdataReqSlot]) -> Vec<(TdmaTime, u32, bool)> {
    let mut out: Vec<(TdmaTime, u32, bool)> = slots
        .iter()
        .filter(|s| s.carrier_num == MAIN_CARRIER && s.ts.t == 1)
        .flat_map(|s| {
            mac_resources(s)
                .into_iter()
                .filter_map(move |r| Some((s.ts, r.addr?.ssi, r.chan_alloc_element.is_some())))
        })
        .collect();
    out.sort_by_key(|r| (r.0.diff(TdmaTime::default()), r.1, r.2));
    out
}

/// The call that fills the main carrier switches the secondary on: the next one finds it on the
/// air, its D-CONNECT and D-SETUP go on the MCCH when they would with the secondary always on.
#[test]
fn a_call_overflowing_to_a_warm_carrier_on_demand_does_not_wait() {
    debug::setup_logging_verbose();
    let on_demand = overflowing_group_call(true, true);
    let always = overflowing_group_call(false, true);
    use tetra_saps::control::call_control::CallControl;
    let on_secondary = on_demand
        .msgs
        .iter()
        .any(|m| matches!(&m.msg, SapMsgInner::CmceCallControl(CallControl::Open(c)) if c.carrier_num == SECONDARY_CARRIER));
    assert!(on_secondary, "the call on the secondary carrier");
    let (on_demand, always) = (mcch_resources(&voice_slots(&on_demand)), mcch_resources(&voice_slots(&always)));
    assert!(on_demand.iter().any(|r| r.1 == GSSI && r.2), "the D-SETUP: {on_demand:?}");
    assert_eq!(on_demand, always);
}

/// The call that fills the main carrier needs the secondary at once: the BS acknowledges the
/// U-SETUP at once, but its D-CONNECT and the group's D-SETUP go only once the secondary has been
/// on the air for the warm-up. The call ends as always, and the carrier goes off after it.
#[test]
fn a_call_overflowing_to_a_cold_carrier_on_demand_waits_for_the_warm_up() {
    debug::setup_logging_verbose();
    let mut v = overflowing_group_call(true, false);
    let slots = voice_slots(&v);
    let t_on = slots
        .iter()
        .find(|s| s.carrier_num == SECONDARY_CARRIER)
        .map(|s| s.ts)
        .expect("on the air");
    let mcch = mcch_resources(&slots);
    let first = |ssi: u32, chan_alloc: bool| mcch.iter().find(|r| r.1 == ssi && r.2 == chan_alloc).map(|r| r.0);
    let ack = first(ISSI, false).expect("the U-SETUP acknowledged");
    assert!(ack.diff(t_on) < 16, "at once: on {t_on}, {ack}: {mcch:?}");
    let d_connect = first(ISSI, true).expect("the D-CONNECT");
    let d_setup = first(GSSI, true).expect("the D-SETUP");
    assert!(d_connect.diff(t_on) >= 16, "D-CONNECT after the warm-up: on {t_on}, {d_connect}");
    assert!(d_setup.diff(t_on) >= 16, "D-SETUP after the warm-up: on {t_on}, {d_setup}");
    let always = mcch_resources(&voice_slots(&overflowing_group_call(false, false)));
    let d_connect_always = always.iter().find(|r| r.1 == ISSI && r.2).map(|r| r.0).expect("the D-CONNECT");
    assert!(
        d_connect_always.diff(t_on) < 16,
        "with the secondary always on it goes at once: {d_connect_always}"
    );
    println!("on {t_on}, acknowledged {ack}, D-CONNECT {d_connect} (always on: {d_connect_always}), D-SETUP {d_setup}");

    let call_id = v.d_setup_call_id(GSSI);
    v.cmce(ISSI, u_tx_ceased(call_id));
    // Hangtime (5 s) and the D-RELEASE.
    v.run(500);
    assert!(count(&v.log, "CallEnded") >= 1, "the call ended after the hangtime");
    let last = |v: &Voice| -> Vec<u16> { voice_slots(v).iter().rev().take(40).map(|s| s.carrier_num).collect() };
    assert!(
        last(&v).contains(&SECONDARY_CARRIER),
        "still on: the main carrier is full, the next call goes there"
    );
    {
        let mut state = v.test.config.state_write();
        for ts in 2..=4 {
            state.timeslot_alloc.release(TimeslotOwner::Brew, ts).unwrap();
        }
    }
    // The grace (1 s).
    v.run(120);
    assert!(
        last(&v).iter().all(|c| *c == MAIN_CARRIER),
        "off once nothing needs it: {:?}",
        last(&v)
    );
}
