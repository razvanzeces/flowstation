//! SNDCP packet-data runtime for `[packet_data]` (EN 300 392-2 clause 28): PDP contexts with an
//! IPv4 address from the pool (the ACCEPT keeps the stub's CHAP Success for DIMETRA radios),
//! READY and STANDBY, SN-DATA TRANSMIT REQUEST / RESPONSE, SN-RECONNECT, SN-END OF DATA, and the
//! radios' datagrams to and from the WAP gateway on the common control channel (no channel is
//! assigned): SN-UNITDATA on the unacknowledged basic link, SN-DATA on the acknowledged advanced
//! link. Each answer goes back on the bearer the radio's last datagram came on.
//!
//! Voice first: nothing goes down to a radio while it is in a call (its gateway timers wait), and
//! at most one data PDU of the whole runtime waits in the MAC for the MCCH, so a call set-up waits
//! behind one data PDU at most. An SN-DATA out of the MAC and not acknowledged yet holds back only
//! its own radio's next datagram.
//!
//! With `bearer = "pdch"` the SN-DATA TRANSMIT RESPONSE assigns the radio a packet-data channel
//! (PDCH): one timeslot of the main carrier (`pdch_timeslots`), or up to `pdch_max_slots` of them
//! for a multislot radio, held in the timeslot allocator as `PacketData` and published in
//! `StackState::pdch_by_issi` (and `pdch_timeslots_by_issi`) for the MAC (AACH) and the LLC
//! (routing). Voice takes a slot when nothing else is free, and with it the whole channel; the
//! radio then sees the AACH change and returns to the MCCH by itself, where its data goes on. A
//! channel of several slots leaves one main-carrier traffic slot free for voice when no other
//! carrier has one. The channel is given back with
//! SN-END OF DATA (quit and go back to the MCCH, STANDBY) when READY ends or after
//! `pdch_idle_release_secs` without data, and without signalling when the radio goes into a call,
//! reconnects from the MCCH without data (it is in STANDBY already) or its contexts end. With no
//! slot free the data stays on the MCCH.
//!
//! The SNDCP timers run on the TDMA clock (one tick per timeslot); the gateway keeps wall-clock
//! time for WTP.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::Ipv4Addr;
use std::time::Instant;

use tetra_config::bluestation::{PacketDataBearer, PdchGrant, SharedConfig, StackState, ready_timer_ms};
use tetra_core::tetra_entities::TetraEntity;
use tetra_core::{BitBuffer, CarrierSlot, Sap, TetraAddress, TimeslotOwner, TxReporter, TxState};
use tetra_saps::lcmc::enums::alloc_type::ChanAllocType;
use tetra_saps::lcmc::enums::ul_dl_assignment::UlDlAssignment;
use tetra_saps::lcmc::fields::chan_alloc_req::CmceChanAllocReq;
use tetra_saps::ltpd::{LtpdBearer, LtpdMleUnitdataInd};
use tetra_saps::tla::{TlDataReqAl, TlaTlUnitdataReqBl};
use tetra_saps::tma::DATA_CATEGORY_PDCH_ASSIGNMENT;
use tetra_saps::{SapMsg, SapMsgInner};

use super::ip::{bitbuffer_npdu_octets, parse_ipv4_packet};
use super::sndcp_bs::{
    Deactivation, MLE_DISCRIMINATOR_SNDCP, TIA_IPV4_DYNAMIC, TIA_IPV4_STATIC, decode_deactivate_demand, encode_deactivate_accept,
    encode_pdp_accept, encode_pdp_reject, parse_demand, tl_data_req,
};
use super::transfer::{
    SndcpDataTransmitResponse, SndcpDataTransmitResponseResult, SndcpEndOfData, SndcpNotSupported, SndcpPacketDataResourceRequest,
    SndcpTransferRejectCause, decode_data_transmit_request, decode_reconnect, encode_data_transmit_response, encode_end_of_data,
    encode_not_supported,
};
use super::unitdata::{SNDCP_NO_COMPRESSION, decode_sn_data_pdu, decode_sn_unitdata_pdu, encode_sn_data, encode_sn_unitdata};
use super::wapgw::{UdpOut, WapService, WapVia};
use crate::MessageQueue;

/// Timeslots in `ms` milliseconds (a timeslot lasts 85/6 ms).
const fn slots(ms: u64) -> u64 {
    (ms * 6).div_ceil(85)
}

/// Housekeeping (timers, registrations, calls) about once a second.
const HOUSEKEEPING_SLOTS: u64 = 72;
/// The station's READY timer runs this much shorter than the one announced (clause 28.2.6.2
/// NOTE 1: the SwMI ends READY first).
const READY_MARGIN_MS: u64 = 2_000;
/// STANDBY timer announced in the ACCEPT (code 5 = 10 min, table 28.122).
const STANDBY_MS: u64 = 600_000;
/// Downlink datagrams waiting for one context; the oldest goes when full.
const MAX_DL_QUEUE: usize = 16;
/// A data PDU the MAC never reports on stops holding the MCCH after this long.
const INFLIGHT_GUARD_SLOTS: u64 = slots(10_000);
/// Largest datagram sent back in one SN-UNITDATA on the basic link: N.251 (2595 bits without
/// FCS) less the MLE discriminator and the SN-UNITDATA header, with a margin.
const BL_MAX_DATAGRAM: usize = 320;
/// What N.271 holds besides the datagram: the AL FCS (4 octets), the SN-DATA header (2) and the
/// MLE discriminator (3 bits, one octet).
const AL_OVERHEAD: usize = 7;
/// Bound on PDP contexts (dynamic pool plus static addresses).
const MAX_CONTEXTS: usize = 2048;
/// A main-carrier slot freed by a call or by another PDCH shows the AACH unallocated this long
/// (one multiframe) before it becomes a PDCH: a radio still on it (hangtime, a missed D-RELEASE or
/// SN-END OF DATA) sees that and leaves (clause 23.5.6.1.1), where assigned control looks alike.
const PDCH_QUARANTINE_SLOTS: u64 = 72;
/// Radios remembered for the one-warning-per-radio rule on malformed PDUs.
const MAX_WARNED: usize = 1024;

// SN-PDU types (clause 28.4.5, "SN PDU type").
const SN_ACTIVATE_PDP_CONTEXT: u8 = 0;
const SN_DEACTIVATE_PDP_CONTEXT_ACCEPT: u8 = 1;
const SN_DEACTIVATE_PDP_CONTEXT_DEMAND: u8 = 2;
const SN_ACTIVATE_PDP_CONTEXT_REJECT: u8 = 3;
const SN_UNITDATA: u8 = 4;
const SN_DATA: u8 = 5;
const SN_DATA_TRANSMIT_REQUEST: u8 = 6;
const SN_DATA_TRANSMIT_RESPONSE: u8 = 7;
const SN_END_OF_DATA: u8 = 8;
const SN_RECONNECT: u8 = 9;
const SN_PAGE: u8 = 10;
const SN_NOT_SUPPORTED: u8 = 11;
const SN_DATA_PRIORITY: u8 = 12;
const SN_MODIFY: u8 = 13;

// Activation reject causes (table 28.46).
const REJECT_UNDEFINED: u8 = 0;
const REJECT_POOL_EMPTY: u8 = 7;
const REJECT_STATIC_NOT_CORRECT: u8 = 8;
const REJECT_STATIC_IN_USE: u8 = 9;
const REJECT_STATIC_NOT_ALLOWED: u8 = 10;
const REJECT_MAX_CONTEXTS: u8 = 19;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CtxState {
    Standby,
    Ready,
}

#[derive(Debug, Default, Clone, Copy)]
struct Counters {
    up: u32,
    up_bytes: usize,
    down: u32,
    down_bytes: usize,
}

struct Ctx {
    addr: TetraAddress,
    ip: Ipv4Addr,
    state: CtxState,
    /// Timeslot at which the running READY or STANDBY timer expires.
    deadline: u64,
    /// IPv4 N-PDUs waiting to go down.
    dl: VecDeque<Vec<u8>>,
    /// The radio was seen registered: losing the registration then releases the context.
    seen_registered: bool,
    in_call: bool,
    /// Traffic since the context last entered READY.
    counters: Counters,
    /// Bearer of the last datagram from the radio, which its answers take.
    reply_bearer: LtpdBearer,
}

struct Inflight {
    key: (u32, u8),
    reporter: TxReporter,
    since: u64,
}

/// A radio's packet-data channel.
struct Pdch {
    /// Its timeslots, the one of its grant (where its PDUs are sent) first.
    slots: Vec<CarrierSlot>,
    /// The SN-DATA TRANSMIT RESPONSE carrying the assignment, until it went out.
    assignment: Option<TxReporter>,
    /// The same once it went out, until the radio acknowledges it: lost, the radio never got it.
    assignment_ack: Option<TxReporter>,
    /// The assignment went out: the radio is on the channel (as far as the station knows).
    on_air: bool,
    /// Last data or transfer request of the radio (TDMA clock).
    last_activity: u64,
    /// The SN-END OF DATA sending the radio back to the MCCH: the channel goes once it went out.
    quit: Option<TxReporter>,
}

/// `[packet_data] bearer = "pdch"`.
struct PdchConfig {
    main_carrier: u16,
    prefs: Vec<u8>,
    idle_slots: u64,
    /// Timeslots a radio's channel may have (`StackConfig::pdch_slots_per_radio`).
    max_slots: u8,
    /// The packet-data carrier in use (`[packet_data] pdch_carrier`).
    carrier: Option<CarrierCfg>,
}

/// The packet-data carrier: its number, the slots a channel may take there in order of
/// preference, and whether voice never goes there.
struct CarrierCfg {
    num: u16,
    prefs: Vec<u8>,
    exclusive: bool,
}

pub struct PacketDataRuntime {
    pool_first: u32,
    pool_last: u32,
    gateway: Ipv4Addr,
    mtu: u16,
    mtu_code: u8,
    ready_code: u8,
    ready_slots: u64,
    standby_slots: u64,
    ctxs: HashMap<(u32, u8), Ctx>,
    by_ip: HashMap<Ipv4Addr, (u32, u8)>,
    /// Timeslots since start.
    clock: u64,
    next_housekeeping: u64,
    /// The data PDU waiting in the MAC for the MCCH.
    inflight: Option<Inflight>,
    /// SN-DATA that left the MAC, waiting for the radio's acknowledgement on its advanced link, by
    /// ISSI: only that radio's next datagram waits for it.
    awaiting_ack: HashMap<u32, Inflight>,
    last_served: Option<(u32, u8)>,
    /// ISSIs whose gateway retransmission timers are paused.
    paused: HashSet<u32>,
    warned: HashSet<u32>,
    ip_identification: u16,
    /// Some with `bearer = "pdch"`.
    pdch_cfg: Option<PdchConfig>,
    /// Packet-data channels by ISSI.
    pdch: HashMap<u32, Pdch>,
    /// Last tick each main-carrier timeslot (index) had an owner.
    slot_busy_at: [Option<u64>; 5],
    /// The same for the slots of the packet-data carrier.
    carrier_busy_at: [Option<u64>; 5],
    main_carrier: u16,
    /// Radios that stayed on the MCCH after an assignment to the packet-data carrier: main-carrier
    /// channels only for them from then on.
    carrier_refused: HashSet<u32>,
}

/// MTU code of the ACCEPT (table 28.79); `[wap] mtu` only takes these values.
fn mtu_code(mtu: u16) -> u8 {
    match mtu {
        296 => 1,
        1006 => 3,
        1500 => 4,
        2002 => 5,
        _ => 2,
    }
}

/// The SN-PDU prefixed with the MLE discriminator for SNDCP, as the MS's MLE expects it.
fn mle_sdu(pdu: &BitBuffer) -> BitBuffer {
    let mut pdu = BitBuffer::from_bitbuffer(pdu);
    pdu.seek(0);
    let len = pdu.get_len();
    let mut sdu = BitBuffer::new(3 + len);
    sdu.write_bits(MLE_DISCRIMINATOR_SNDCP, 3);
    sdu.copy_bits(&mut pdu, len);
    sdu.seek(0);
    sdu
}

fn field(bits: &str, off: usize, n: usize) -> Option<u8> {
    bits.get(off..off + n).and_then(|s| u8::from_str_radix(s, 2).ok())
}

fn valid_nsapi(nsapi: u8) -> bool {
    (1..=14).contains(&nsapi)
}

/// Whether `issi` is on a traffic channel: in an individual call, or affiliated to a group with a
/// call up (the same live map the SDS path uses).
pub(crate) fn in_call(state: &StackState, issi: u32) -> bool {
    state.active_call_ts.contains_key(&issi)
        || state
            .subscribers
            .attached_groups_of(issi)
            .iter()
            .any(|g| state.active_call_ts.contains_key(g))
}

/// "Timeslot assigned" of the slots of a packet-data channel (clause 21.5.2, a bitmap).
fn timeslot_bits(slots: &[CarrierSlot]) -> [bool; 4] {
    let mut timeslots = [false; 4];
    for slot in slots {
        timeslots[usize::from(slot.ts.clamp(1, 4)) - 1] = true;
    }
    timeslots
}

/// Channel allocation of a packet-data channel: `slots` (of the first one's carrier), both
/// directions, replacing the MCCH.
fn pdch_assignment(slots: &[CarrierSlot]) -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: None,
        carrier: slots.first().map(|s| s.carrier_num),
        timeslots: timeslot_bits(slots),
        alloc_type: ChanAllocType::Replace,
        ul_dl_assigned: UlDlAssignment::Both,
    }
}

/// The timeslots of a packet-data channel for the log: "4", or "4+3".
fn ts_list(slots: &[CarrierSlot]) -> String {
    slots.iter().map(|s| s.ts.to_string()).collect::<Vec<_>>().join("+")
}

/// `ts_list`, with " of carrier N" for a channel of another carrier than `main_carrier`.
fn ts_on(slots: &[CarrierSlot], main_carrier: u16) -> String {
    match slots.first() {
        Some(s) if s.carrier_num != main_carrier => format!("{} of carrier {}", ts_list(slots), s.carrier_num),
        _ => ts_list(slots),
    }
}

/// Timeslots per TDMA frame the radio can take (clause 28.4.5.34, table 28.115): its full phase
/// modulation capability, which also bounds a channel wider than it asked for (28.3.4.8 NOTE 1);
/// 1 without a resource request. The count it asks is no limit: after its first session the
/// radio asks the N.264 of its advanced link (28.3.5.2a).
fn slots_capable(request: &SndcpPacketDataResourceRequest) -> u8 {
    match request {
        SndcpPacketDataResourceRequest::None => 1,
        SndcpPacketDataResourceRequest::PhaseModulation(r) => r.full_phase_modulation_capability_timeslots.max(1),
    }
}

/// Channel allocation that sends the radio from its packet-data channel back to the MCCH.
fn quit_to_mcch(main_carrier: u16) -> CmceChanAllocReq {
    CmceChanAllocReq {
        usage: None,
        carrier: Some(main_carrier),
        timeslots: [false; 4],
        alloc_type: ChanAllocType::QuitAndGo,
        ul_dl_assigned: UlDlAssignment::Both,
    }
}

/// `msg` (a TL-DATA request) with a channel allocation and a report of its transmission. It is
/// marked as a packet-data channel assignment, so the MAC drops a channel allocation that is no
/// longer the radio's when it goes out.
fn with_chan_alloc(mut msg: SapMsg, chan_alloc: CmceChanAllocReq, reporter: &TxReporter) -> SapMsg {
    if let SapMsgInner::TlaTlDataReqBl(req) = &mut msg.msg {
        req.chan_alloc = Some(chan_alloc);
        req.data_class_info = Some(DATA_CATEGORY_PDCH_ASSIGNMENT);
        req.tx_reporter = Some(reporter.clone());
    }
    msg
}

/// The resource request for the log; with `full` its full capability too.
fn resources(request: &SndcpPacketDataResourceRequest, full: bool) -> String {
    match request {
        SndcpPacketDataResourceRequest::None => "no resource request".to_string(),
        SndcpPacketDataResourceRequest::PhaseModulation(r) => format!(
            "asks {} UL / {} DL slot(s){}{}",
            r.uplink_timeslots,
            r.downlink_timeslots,
            if r.unspecified_phase_modulation_resource {
                ", unspecified"
            } else {
                ""
            },
            if full {
                format!(", full {}", r.full_phase_modulation_capability_timeslots)
            } else {
                String::new()
            }
        ),
    }
}

impl PacketDataRuntime {
    /// None unless `[packet_data] enabled`.
    pub fn new(config: &SharedConfig) -> Option<Self> {
        let cfg = config.config();
        let pd = &cfg.packet_data;
        if !pd.enabled {
            return None;
        }
        let announced_ms = ready_timer_ms(pd.ready_timer_code).unwrap_or(30_000);
        let ready_ms = announced_ms.saturating_sub(READY_MARGIN_MS).max(announced_ms / 2);
        if !cfg.cell.sndcp_service {
            tracing::warn!("SNDCP: [packet_data] is on but cell_info sndcp_service = false: radios will not start packet data");
        }
        if !cfg.cell.advanced_link {
            tracing::warn!("SNDCP: [packet_data] is on but cell_info advanced_link = false: some radios need it for packet data");
        }
        if !cfg.wap.enabled {
            tracing::warn!("SNDCP: [packet_data] is on but [wap] is off: the radios' datagrams have nowhere to go");
        }
        let fetch_ms = cfg.wap.wtp.hold_on_ms + cfg.wap.browse.timeout_secs * 1000;
        if cfg.wap.enabled && ready_ms <= fetch_ms {
            tracing::warn!(
                "SNDCP: READY ({} ms) is not longer than a download may take ({} ms): a page may come back after READY ended",
                ready_ms,
                fetch_ms
            );
        }
        // A packet-data carrier that is not in use is said once, never an error.
        let carrier = cfg.pdch_carrier();
        match pd.pdch_carrier {
            Some(c) if carrier.is_none() => {
                if c == cfg.cell.main_carrier {
                    tracing::warn!(
                        "SNDCP: pdch_carrier = {} is the main carrier (its packet-data slots are pdch_timeslots): ignored",
                        c
                    );
                } else if pd.bearer == PacketDataBearer::Mcch {
                    tracing::warn!("SNDCP: pdch_carrier = {} has no effect with bearer = \"mcch\"", c);
                } else if cfg.cell.extra_carriers.is_empty() {
                    tracing::warn!(
                        "SNDCP: pdch_carrier = {} is not the secondary carrier in use ({}): packet data stays on the main carrier's pdch_timeslots",
                        c,
                        cfg.cell
                            .secondary_carrier
                            .map_or("none, dual carrier off".to_string(), |s| s.to_string())
                    );
                } else {
                    tracing::warn!(
                        "SNDCP: pdch_carrier = {} is not a carrier of the cell in use other than the main one ({:?}): packet data stays on the main carrier's pdch_timeslots",
                        c,
                        cfg.cell.other_carriers().collect::<Vec<_>>()
                    );
                }
            }
            None if pd.pdch_carrier_timeslots != [4, 3, 2, 1] || pd.pdch_carrier_exclusive => {
                tracing::warn!("SNDCP: pdch_carrier_timeslots / pdch_carrier_exclusive have no effect without pdch_carrier");
            }
            _ => {}
        }
        let max_slots = cfg.pdch_slots_per_radio();
        if pd.bearer == PacketDataBearer::Pdch && pd.pdch_max_slots > max_slots {
            match &carrier {
                None => tracing::warn!(
                    "SNDCP: pdch_max_slots = {} but pdch_timeslots has {} distinct main-carrier slot(s): at most {} slot(s) per radio (a PDCH on a secondary carrier is not supported)",
                    pd.pdch_max_slots,
                    pd.pdch_timeslot_count(),
                    max_slots
                ),
                Some(c) => tracing::warn!(
                    "SNDCP: pdch_max_slots = {} but pdch_timeslots {:?} and pdch_carrier_timeslots {:?} give at most {} slot(s) per radio",
                    pd.pdch_max_slots,
                    pd.pdch_timeslots,
                    c.timeslots,
                    max_slots
                ),
            }
        }
        if pd.bearer == PacketDataBearer::Mcch && pd.pdch_max_slots > 1 {
            tracing::warn!("SNDCP: pdch_max_slots = {} has no effect with bearer = \"mcch\"", pd.pdch_max_slots);
        }
        let pdch_cfg = (pd.bearer == PacketDataBearer::Pdch).then(|| PdchConfig {
            main_carrier: cfg.cell.main_carrier,
            prefs: pd.pdch_timeslots.clone(),
            idle_slots: slots(u64::from(pd.pdch_idle_release_secs) * 1000),
            max_slots,
            carrier: carrier.map(|c| CarrierCfg {
                num: c.carrier,
                prefs: c.timeslots,
                exclusive: c.exclusive,
            }),
        });
        tracing::info!(
            "SNDCP: packet data on {} (pool {}..{}, gateway {}, MTU {}, READY {} ms announced / {} ms here)",
            match &pdch_cfg {
                Some(p) => format!(
                    "{}{}, released after {} s idle, else the MCCH",
                    match &p.carrier {
                        None => format!("a PDCH of main-carrier ts {:?}", p.prefs),
                        Some(c) => format!(
                            "a PDCH of carrier {} ts {:?} (uplink never on ts1; {}), else main-carrier ts {:?}",
                            c.num,
                            c.prefs,
                            if c.exclusive {
                                "data only, voice never there"
                            } else {
                                "shared with voice"
                            },
                            p.prefs
                        ),
                    },
                    if p.max_slots > 1 {
                        format!(", up to {} slots per radio", p.max_slots)
                    } else {
                        String::new()
                    },
                    pd.pdch_idle_release_secs
                ),
                None => "the MCCH".to_string(),
            },
            pd.pool_first,
            pd.pool_last,
            cfg.wap.gateway_ipv4,
            cfg.wap.mtu,
            announced_ms,
            ready_ms
        );
        Some(Self {
            pool_first: u32::from(pd.pool_first),
            pool_last: u32::from(pd.pool_last),
            gateway: cfg.wap.gateway_ipv4,
            mtu: cfg.wap.mtu,
            mtu_code: mtu_code(cfg.wap.mtu),
            ready_code: pd.ready_timer_code,
            ready_slots: slots(ready_ms),
            standby_slots: slots(STANDBY_MS),
            ctxs: HashMap::new(),
            by_ip: HashMap::new(),
            clock: 0,
            next_housekeeping: HOUSEKEEPING_SLOTS,
            inflight: None,
            awaiting_ack: HashMap::new(),
            last_served: None,
            paused: HashSet::new(),
            warned: HashSet::new(),
            ip_identification: 0,
            pdch_cfg,
            pdch: HashMap::new(),
            slot_busy_at: [None; 5],
            carrier_busy_at: [None; 5],
            main_carrier: cfg.cell.main_carrier,
            carrier_refused: HashSet::new(),
        })
    }

    /// Largest datagram the gateway may send back on `bearer`: the MTU, and N.251 on the basic
    /// link or the negotiated N.271 on the advanced link.
    fn max_reply_bytes(&self, bearer: LtpdBearer) -> usize {
        let bearer_max = match bearer {
            LtpdBearer::Advanced { max_sdu_bytes, .. } => usize::from(max_sdu_bytes).saturating_sub(AL_OVERHEAD),
            LtpdBearer::BasicAck | LtpdBearer::BasicUnack => BL_MAX_DATAGRAM,
        };
        usize::from(self.mtu).min(bearer_max)
    }

    /// One uplink SN-PDU (the SDU still carries the 3-bit MLE discriminator).
    pub fn rx(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let raw = ind.sdu.dump_bin_unformatted();
        let bits = raw.get(3..).unwrap_or("");
        let sn_type = field(bits, 0, 4);
        tracing::debug!(
            "SNDCP: <- ISSI {} SN-PDU type {:?}, {} bits, {:?}",
            issi,
            sn_type,
            bits.len(),
            ind.bearer
        );
        // A radio of a channel of the packet-data carrier the LLC heard there again (a stale "back
        // on the MCCH" undone): its answers go there.
        if let Some(p) = self.pdch.get_mut(&issi)
            && !p.on_air
            && p.assignment.is_none()
            && p.slots.first().is_some_and(|s| s.carrier_num != self.main_carrier)
            && config.state_read().pdch_by_issi.get(&issi).is_some_and(|g| g.on_air)
        {
            p.on_air = true;
            tracing::info!("SNDCP: ISSI {} is back on its PDCH ts {}", issi, ts_on(&p.slots, self.main_carrier));
        }
        // Anything from the radio on the MCCH: it left its packet-data channel. (A channel of the
        // packet-data carrier never transmits on its ts1: link 1 is always the MCCH.)
        if ind.link_id == 1 && self.pdch.get(&issi).is_some_and(|p| p.on_air) {
            if let Some(carrier) = self.refused_its_carrier(config, issi) {
                self.stayed_on_the_mcch(config, issi, carrier);
            } else if let Some(p) = self.pdch.get_mut(&issi) {
                p.on_air = false;
                tracing::info!(
                    "SNDCP: ISSI {} is back on the MCCH, its PDCH ts {} is kept for now",
                    issi,
                    ts_on(&p.slots, self.main_carrier)
                );
                Self::publish_pdch(config, issi, Some(p));
            }
        }
        match sn_type {
            Some(SN_ACTIVATE_PDP_CONTEXT) => self.on_demand(queue, ind, bits, wap),
            Some(SN_DEACTIVATE_PDP_CONTEXT_DEMAND) => self.on_deactivate(queue, ind, bits, wap, config),
            Some(SN_UNITDATA) => self.on_user_data(ind, bits, wap, config, now),
            Some(SN_DATA) if matches!(ind.bearer, LtpdBearer::Advanced { .. }) => self.on_user_data(ind, bits, wap, config, now),
            Some(SN_DATA) => tracing::debug!(
                "SNDCP: SN-DATA from ISSI {} on the basic link (table 28.16 allows it only on the advanced link), dropped",
                issi
            ),
            Some(SN_DATA_TRANSMIT_REQUEST) => self.on_transmit_request(queue, ind, bits, wap, config, now),
            Some(SN_END_OF_DATA) => self.on_end_of_data(queue, ind, bits, wap, config, now),
            Some(SN_RECONNECT) => self.on_reconnect(queue, ind, bits, wap, config, now),
            Some(t @ (SN_PAGE | SN_DATA_PRIORITY | SN_MODIFY)) => {
                tracing::info!("SNDCP: SN-PDU type {} from ISSI {} not supported, SN-NOT SUPPORTED sent", t, issi);
                if let Ok(pdu) = encode_not_supported(&SndcpNotSupported { not_supported_pdu_type: t }) {
                    queue.push_back(tl_data_req(ind.received_tetra_address, ind.link_id, ind.endpoint_id, mle_sdu(&pdu)));
                }
            }
            Some(SN_NOT_SUPPORTED) => {
                tracing::info!("SNDCP: ISSI {} does not support SN-PDU type {:?}", issi, field(bits, 4, 4));
            }
            Some(SN_DEACTIVATE_PDP_CONTEXT_ACCEPT | SN_ACTIVATE_PDP_CONTEXT_REJECT | SN_DATA_TRANSMIT_RESPONSE) => {
                tracing::debug!(
                    "SNDCP: ISSI {} sent SN-PDU type {:?}, which only the station sends; dropped",
                    issi,
                    sn_type
                );
            }
            _ => self.warn_malformed(issi, "unknown SN-PDU type"),
        }
    }

    fn warn_malformed(&mut self, issi: u32, what: &str) {
        if self.warned.len() >= MAX_WARNED {
            self.warned.clear();
        }
        if self.warned.insert(issi) {
            tracing::warn!(
                "SNDCP: malformed SN-PDU from ISSI {} ({}), dropped (further ones logged at debug)",
                issi,
                what
            );
        } else {
            tracing::debug!("SNDCP: malformed SN-PDU from ISSI {} ({}), dropped", issi, what);
        }
    }

    fn has_ctx(&self, issi: u32) -> bool {
        self.ctxs.keys().any(|k| k.0 == issi)
    }

    /// Lowest free pool address.
    fn allocate(&self) -> Option<Ipv4Addr> {
        (self.pool_first..=self.pool_last)
            .map(Ipv4Addr::from)
            .find(|ip| *ip != self.gateway && !self.by_ip.contains_key(ip))
    }

    fn remove_ctx(&mut self, key: (u32, u8)) -> Option<Ctx> {
        let ctx = self.ctxs.remove(&key)?;
        if self.by_ip.get(&ctx.ip) == Some(&key) {
            self.by_ip.remove(&ctx.ip);
        }
        Some(ctx)
    }

    /// The radio has no context left: the gateway forgets it and its packet-data channel goes.
    fn release_issi(&mut self, issi: u32, wap: Option<&mut WapService>, config: &SharedConfig) {
        if !self.has_ctx(issi) {
            self.forget_issi(issi, wap, config);
        }
    }

    /// `release_issi` for a radio known to have no context left.
    fn forget_issi(&mut self, issi: u32, wap: Option<&mut WapService>, config: &SharedConfig) {
        self.release_pdch(config, issi, "no PDP context left");
        self.paused.remove(&issi);
        self.awaiting_ack.remove(&issi);
        if let Some(wap) = wap {
            wap.peer_lost(issi);
        }
    }

    /// Pause the gateway's retransmission timers of `issi` while nothing can go down to it
    /// (STANDBY, or in a call), and resume them when something can.
    fn sync_pause(&mut self, issi: u32, wap: Option<&mut WapService>, now: Instant) {
        let Some(wap) = wap else { return };
        if !self.has_ctx(issi) {
            return;
        }
        let reachable = self
            .ctxs
            .iter()
            .any(|(k, c)| k.0 == issi && c.state == CtxState::Ready && !c.in_call);
        self.set_paused(issi, !reachable, wap, now);
    }

    fn set_paused(&mut self, issi: u32, pause: bool, wap: &mut WapService, now: Instant) {
        if !pause {
            if self.paused.remove(&issi) {
                wap.resume_timers(issi, now);
            }
        } else if self.paused.insert(issi) {
            wap.pause_timers(issi);
        }
    }

    fn on_demand(&mut self, queue: &mut MessageQueue, ind: &LtpdMleUnitdataInd, bits: &str, mut wap: Option<&mut WapService>) {
        let addr = ind.received_tetra_address;
        let issi = addr.ssi;
        // Type, version, NSAPI and ATID at least.
        if bits.len() < 15 {
            self.warn_malformed(issi, "short SN-ACTIVATE PDP CONTEXT DEMAND");
            return;
        }
        let demand = parse_demand(bits);
        let nsapi = demand.nsapi;
        let reply = |pdu: BitBuffer| tl_data_req(addr, ind.link_id, ind.endpoint_id, pdu);
        let key = (issi, nsapi);
        let replaced = self.remove_ctx(key).is_some();
        if replaced {
            tracing::info!("SNDCP: ISSI {} NSAPI {} activates again, the old context is released", issi, nsapi);
            if let Some(wap) = wap.as_deref_mut() {
                wap.peer_lost(issi);
            }
            self.paused.remove(&issi);
        }
        let grant = if !valid_nsapi(nsapi) {
            Err((REJECT_UNDEFINED, "reserved NSAPI"))
        } else if self.ctxs.len() >= MAX_CONTEXTS {
            Err((REJECT_MAX_CONTEXTS, "too many PDP contexts"))
        } else if demand.atid == 0 {
            match demand.static_ipv4.map(Ipv4Addr::from) {
                None => Err((REJECT_STATIC_NOT_CORRECT, "static address missing")),
                Some(ip) if ip == self.gateway => Err((REJECT_STATIC_NOT_ALLOWED, "static address is the gateway's")),
                Some(ip) if self.by_ip.get(&ip).is_some_and(|k| *k != key) => Err((REJECT_STATIC_IN_USE, "static address in use")),
                Some(ip) => Ok((TIA_IPV4_STATIC, ip)),
            }
        } else {
            self.allocate()
                .map(|ip| (TIA_IPV4_DYNAMIC, ip))
                .ok_or((REJECT_POOL_EMPTY, "pool empty"))
        };
        let (tia, ip) = match grant {
            Ok(grant) => grant,
            Err((cause, why)) => {
                tracing::info!("SNDCP: PDP reject ISSI {} NSAPI {} (cause {}, {})", issi, nsapi, cause, why);
                queue.push_back(reply(encode_pdp_reject(nsapi, cause)));
                return;
            }
        };
        let static_note = if tia == TIA_IPV4_STATIC {
            if (self.pool_first..=self.pool_last).contains(&u32::from(ip)) {
                "static"
            } else {
                "static, outside the pool"
            }
        } else {
            "dynamic"
        };
        tracing::info!(
            "SNDCP: PDP activate ISSI {} NSAPI {} -> IP {} ({}, {}), MTU {}",
            issi,
            nsapi,
            ip,
            static_note,
            demand
                .chap_id
                .map(|id| format!("CHAP id {id}"))
                .unwrap_or_else(|| "no CHAP".to_string()),
            self.mtu
        );
        queue.push_back(reply(encode_pdp_accept(
            nsapi,
            tia,
            u32::from(ip),
            self.mtu_code,
            self.ready_code,
            demand.chap_id,
        )));
        self.by_ip.insert(ip, key);
        self.ctxs.insert(
            key,
            Ctx {
                addr,
                ip,
                state: CtxState::Standby,
                deadline: self.clock + self.standby_slots,
                dl: VecDeque::new(),
                seen_registered: false,
                in_call: false,
                counters: Counters::default(),
                reply_bearer: LtpdBearer::BasicUnack,
            },
        );
    }

    fn on_deactivate(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        bits: &str,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let Some(deactivation) = decode_deactivate_demand(bits) else {
            self.warn_malformed(issi, "short SN-DEACTIVATE PDP CONTEXT DEMAND");
            return;
        };
        let keys: Vec<(u32, u8)> = match deactivation {
            Deactivation::All => self.ctxs.keys().filter(|k| k.0 == issi).copied().collect(),
            Deactivation::Nsapi(nsapi) => vec![(issi, nsapi)],
        };
        let released = keys.into_iter().filter(|k| self.remove_ctx(*k).is_some()).count();
        tracing::info!(
            "SNDCP: PDP deactivate ISSI {} ({:?}): {} context(s) released",
            issi,
            deactivation,
            released
        );
        queue.push_back(tl_data_req(
            ind.received_tetra_address,
            ind.link_id,
            ind.endpoint_id,
            encode_deactivate_accept(deactivation),
        ));
        if released > 0 {
            self.release_issi(issi, wap, config);
        }
    }

    /// SN-DATA TRANSMIT RESPONSE for `nsapi`: accepted when the context exists, which enters READY,
    /// with the assignment of a packet-data channel (`bearer = "pdch"` and a slot free; up to
    /// `capable` slots, see `slots_capable`) or with no channel assignment (the data stays on the
    /// MCCH).
    #[allow(clippy::too_many_arguments)]
    fn respond_transmit(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        nsapi: u8,
        what: &str,
        detail: &str,
        capable: u8,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let (ready_slots, clock) = (self.ready_slots, self.clock);
        let accepted = self.ctxs.contains_key(&(issi, nsapi));
        // Voice first: a radio in a call (or with a call up in one of its groups) is not sent to a
        // PDCH, which would be taken back within a second; its data waits on the MCCH.
        let (assignment, channel) = if !accepted {
            (None, String::new())
        } else if self.pdch_cfg.is_some() && in_call(&config.state_read(), issi) {
            (None, "no channel (MCCH, the radio is in a call)".to_string())
        } else {
            self.pdch_for_transfer(config, issi, ind.link_id, capable)
        };
        let result = match self.ctxs.get_mut(&(issi, nsapi)) {
            Some(ctx) => {
                if ctx.state != CtxState::Ready {
                    ctx.counters = Counters::default();
                }
                ctx.state = CtxState::Ready;
                ctx.deadline = clock + ready_slots;
                tracing::info!(
                    "SNDCP: {} ISSI {} NSAPI {} ({}) -> RESPONSE accepted, {}, READY",
                    what,
                    issi,
                    nsapi,
                    detail,
                    channel
                );
                SndcpDataTransmitResponseResult::Accepted
            }
            None => {
                tracing::info!(
                    "SNDCP: {} ISSI {} NSAPI {} ({}) -> RESPONSE rejected (no PDP context)",
                    what,
                    issi,
                    nsapi,
                    detail
                );
                SndcpDataTransmitResponseResult::Rejected(SndcpTransferRejectCause::UnknownNsapi)
            }
        };
        if let Ok(pdu) = encode_data_transmit_response(&SndcpDataTransmitResponse { nsapi, result }) {
            let msg = tl_data_req(ind.received_tetra_address, ind.link_id, ind.endpoint_id, mle_sdu(&pdu));
            queue.push_back(match assignment {
                Some((chan_alloc, reporter)) => with_chan_alloc(msg, chan_alloc, &reporter),
                None => msg,
            });
        }
        self.sync_pause(issi, wap, now);
    }

    /// The packet-data channel for a transfer of `issi`, which can take `capable` slots, whose
    /// request came in on timeslot `link_id`: the assignment to send with the RESPONSE (none when
    /// the radio is already on its channel, or with the data on the MCCH), and how the log should
    /// name it.
    ///
    /// A new channel has as many slots as the radio can take, up to `pdch_max_slots`, the first
    /// free ones of `pdch_timeslots` (in their order, contiguous or not: "timeslot assigned" is a
    /// bitmap, clause 23.5.4.1), fewer when fewer are free (28.3.4.8). Voice first: when no other
    /// carrier has a free slot, a channel of several slots leaves one main-carrier traffic slot
    /// free, since voice takes a packet-data slot, and with it the whole channel, only when
    /// nothing else is free.
    ///
    /// With a packet-data carrier in use the channel may be there instead (`pdch_carrier_timeslots`,
    /// ts1 only together with one of ts 2-4, which then comes first: the channel's uplink never
    /// uses ts1): the carrier or the main carrier, whichever gives more slots, the carrier on a
    /// tie (it leaves the main carrier's traffic slots to voice). Shared with voice, a channel
    /// there leaves one of its slots free when the main carrier has none.
    fn pdch_for_transfer(
        &mut self,
        config: &SharedConfig,
        issi: u32,
        link_id: u32,
        capable: u8,
    ) -> (Option<(CmceChanAllocReq, TxReporter)>, String) {
        let Some(pcfg) = &self.pdch_cfg else {
            return (None, "no channel (MCCH)".to_string());
        };
        let max_slots = pcfg.max_slots;
        let clock = self.clock;
        let (prefs, waiting): (Vec<u8>, Vec<u8>) = pcfg.prefs.iter().copied().partition(|ts| {
            self.slot_busy_at
                .get(usize::from(*ts))
                .copied()
                .flatten()
                .is_none_or(|t| clock.saturating_sub(t) >= PDCH_QUARANTINE_SLOTS)
        });
        // The packet-data carrier: its slots not in quarantine (never for a radio that did not go
        // there before).
        let carrier = pcfg.carrier.as_ref().map(|c| {
            let (usable, waiting): (Vec<u8>, Vec<u8>) = c.prefs.iter().copied().partition(|ts| {
                self.carrier_busy_at
                    .get(usize::from(*ts))
                    .copied()
                    .flatten()
                    .is_none_or(|t| clock.saturating_sub(t) >= PDCH_QUARANTINE_SLOTS)
            });
            (c.num, usable, waiting, c.exclusive)
        });
        let refused = self.carrier_refused.contains(&issi);
        self.drain_preempted(config);
        let main_carrier = self.main_carrier;
        if let Some(p) = self.pdch.get_mut(&issi) {
            p.last_activity = clock;
            p.quit = None;
            // The request may come in on any slot of the channel.
            if p.on_air && p.slots.iter().any(|s| u32::from(s.ts) == link_id) {
                return (None, format!("already on its PDCH ts {}", ts_on(&p.slots, main_carrier)));
            }
            let reporter = TxReporter::new();
            p.assignment = Some(reporter.clone());
            p.assignment_ack = None;
            config.state_write().pdch_heard_on_channel.remove(&issi);
            return (
                Some((pdch_assignment(&p.slots), reporter)),
                format!("PDCH ts {} (again)", ts_on(&p.slots, main_carrier)),
            );
        }
        let wanted = usize::from(capable.min(max_slots).max(1));
        let (slots, headroom, carrier_busy) = {
            let mut state = config.state_write();
            // On the packet-data carrier: the free usable slots, the first of ts 2-4 first.
            let mut on_carrier = None;
            if let Some((c, usable, _, exclusive)) = carrier.as_ref().filter(|_| !refused) {
                let mut free: Vec<u8> = usable
                    .iter()
                    .copied()
                    .filter(|ts| state.timeslot_alloc.slot_is_free(CarrierSlot { carrier_num: *c, ts: *ts }))
                    .collect();
                if let Some(i) = free.iter().position(|ts| (2..=4).contains(ts)) {
                    let first = free.remove(i);
                    free.insert(0, first);
                    let mut n = wanted.min(free.len());
                    let mut hr = false;
                    let (main_free, others_free) = state.timeslot_alloc.free_traffic_slots();
                    if !*exclusive && n > 1 && main_free == 0 {
                        let cap = others_free.saturating_sub(1).max(1);
                        if cap < n {
                            n = cap;
                            hr = true;
                        }
                    }
                    on_carrier = Some((*c, free, n, hr));
                }
            }
            let mut n = wanted;
            let mut headroom = false;
            if n > 1 {
                let (main_free, others_free) = state.timeslot_alloc.free_traffic_slots();
                let cap = main_free.saturating_sub(1).max(1);
                if others_free == 0 && cap < n {
                    n = cap;
                    headroom = true;
                }
            }
            let main_gives = n.min(prefs.iter().filter(|ts| state.timeslot_alloc.is_free(**ts)).count());
            let carrier_had_nothing = on_carrier.is_none();
            let slots = match on_carrier {
                Some((c, free, n_c, hr)) if n_c >= main_gives => {
                    headroom = hr;
                    state.timeslot_alloc.reserve_packet_data_slots_on(c, &free, n_c)
                }
                _ => state.timeslot_alloc.reserve_packet_data_slots(&prefs, n),
            };
            if let Some(first) = slots.first() {
                state.pdch_by_issi.insert(
                    issi,
                    PdchGrant {
                        slot: *first,
                        on_air: false,
                    },
                );
                if slots.len() > 1 {
                    state.pdch_timeslots_by_issi.insert(issi, timeslot_bits(&slots));
                } else {
                    state.pdch_timeslots_by_issi.remove(&issi);
                }
                state.pdch_heard_on_channel.remove(&issi);
            }
            // Only when the carrier offered nothing (not when the main carrier simply offered more).
            let carrier_busy = carrier
                .as_ref()
                .filter(|_| !refused && carrier_had_nothing && slots.first().is_some_and(|s| s.carrier_num == main_carrier));
            (slots, headroom, carrier_busy.map(|c| c.0))
        };
        if slots.is_empty() {
            match &carrier {
                None => tracing::info!(
                    "SNDCP: no main-carrier slot of {:?} free for a PDCH of ISSI {} ({:?} busy or freed less than a multiframe ago), data on the MCCH",
                    prefs,
                    issi,
                    waiting
                ),
                Some((c, usable, c_waiting, _)) => tracing::info!(
                    "SNDCP: no slot of {:?} or of carrier {} {:?} free for a PDCH of ISSI {} ({:?} and {:?} busy or freed less than a multiframe ago), data on the MCCH",
                    prefs,
                    c,
                    usable,
                    issi,
                    waiting,
                    c_waiting
                ),
            }
            return (None, "no channel (MCCH, no PDCH slot free)".to_string());
        }
        tracing::info!(
            "SNDCP: PDCH ts {} reserved for ISSI {}{}{}",
            ts_on(&slots, main_carrier),
            issi,
            if max_slots > 1 {
                format!(
                    " ({} of {} possible{})",
                    slots.len(),
                    capable,
                    if headroom { ", voice headroom" } else { "" }
                )
            } else {
                String::new()
            },
            carrier_busy.map_or(String::new(), |c| format!(", carrier {c} busy"))
        );
        let reporter = TxReporter::new();
        let assignment = pdch_assignment(&slots);
        let channel = format!("PDCH ts {}", ts_on(&slots, main_carrier));
        self.pdch.insert(
            issi,
            Pdch {
                slots,
                assignment: Some(reporter.clone()),
                assignment_ack: None,
                on_air: false,
                last_activity: clock,
                quit: None,
            },
        );
        (Some((assignment, reporter)), channel)
    }

    /// The packet-data carrier when `issi` has a channel there, acknowledged its assignment and
    /// was not heard on the channel since (see `stayed_on_the_mcch`). An assignment only sent
    /// proves nothing: the radio that missed it sends its request again on the MCCH, and gets the
    /// assignment again there (the LLC repeats it).
    fn refused_its_carrier(&self, config: &SharedConfig, issi: u32) -> Option<u16> {
        let p = self.pdch.get(&issi)?;
        let carrier = p.slots.first()?.carrier_num;
        let acknowledged = p.assignment.is_none() && p.assignment_ack.as_ref().is_none_or(|r| r.get_state() == TxState::Acknowledged);
        (carrier != self.main_carrier && acknowledged && !config.state_read().pdch_heard_on_channel.contains(&issi)).then_some(carrier)
    }

    /// A radio that acknowledged its assignment to a channel of the packet-data carrier is heard
    /// on the MCCH before anything of it was heard on the channel: it did not go there (its
    /// acknowledgement of the assignment on the MCCH tells nothing). Its channel is released, and
    /// it gets channels of the main carrier (or the MCCH) from then on.
    fn stayed_on_the_mcch(&mut self, config: &SharedConfig, issi: u32, carrier: u16) {
        self.carrier_refused.insert(issi);
        tracing::info!(
            "SNDCP: ISSI {} stayed on the MCCH after a carrier assignment: main-carrier packet-data channels only from now on",
            issi
        );
        self.release_pdch(config, issi, &format!("the radio did not take its PDCH of carrier {carrier}"));
    }

    /// Publish the packet-data channel of `issi` (None: removed) for the MAC and the LLC: its
    /// grant and, for a channel of several slots, its timeslots, together.
    fn publish_pdch(config: &SharedConfig, issi: u32, pdch: Option<&Pdch>) {
        let mut state = config.state_write();
        match pdch.filter(|p| !p.slots.is_empty()) {
            Some(p) => {
                state.pdch_by_issi.insert(
                    issi,
                    PdchGrant {
                        slot: p.slots[0],
                        on_air: p.on_air,
                    },
                );
                if p.slots.len() > 1 {
                    state.pdch_timeslots_by_issi.insert(issi, timeslot_bits(&p.slots));
                } else {
                    state.pdch_timeslots_by_issi.remove(&issi);
                }
            }
            None => {
                state.pdch_by_issi.remove(&issi);
                state.pdch_timeslots_by_issi.remove(&issi);
            }
        }
    }

    /// Give back the packet-data channel of `issi`, if it has one: every slot of it.
    fn release_pdch(&mut self, config: &SharedConfig, issi: u32, why: &str) {
        let Some(p) = self.pdch.remove(&issi) else { return };
        let errors: Vec<_> = {
            let mut state = config.state_write();
            state.pdch_by_issi.remove(&issi);
            state.pdch_timeslots_by_issi.remove(&issi);
            p.slots
                .iter()
                .filter_map(|s| state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, *s).err())
                .collect()
        };
        match errors.as_slice() {
            [] => tracing::info!(
                "SNDCP: PDCH ts {} of ISSI {} released ({})",
                ts_on(&p.slots, self.main_carrier),
                issi,
                why
            ),
            [e] => tracing::debug!(
                "SNDCP: PDCH ts {} of ISSI {} gone ({}), slot not ours: {:?}",
                ts_on(&p.slots, self.main_carrier),
                issi,
                why,
                e
            ),
            errors => tracing::debug!(
                "SNDCP: PDCH ts {} of ISSI {} gone ({}), slots not ours: {:?}",
                ts_on(&p.slots, self.main_carrier),
                issi,
                why,
                errors
            ),
        }
    }

    /// Packet-data channels voice took a slot of: the radio goes back to the MCCH on its own
    /// (AACH), and the whole channel goes (its other slots back to the pool). Also run before
    /// every reservation, so a slot taken earlier is never mistaken for a new one.
    fn drain_preempted(&mut self, config: &SharedConfig) {
        if self.pdch_cfg.is_none() {
            return;
        }
        let taken = {
            let mut state = config.state_write();
            let taken = state.timeslot_alloc.drain_preempted_packet_data();
            for slot in &taken {
                let gone: Vec<u32> = state
                    .pdch_by_issi
                    .iter()
                    .filter(|(issi, g)| {
                        g.slot == *slot
                            || (g.slot.carrier_num == slot.carrier_num
                                && state
                                    .pdch_timeslots_by_issi
                                    .get(*issi)
                                    .is_some_and(|bits| bits[usize::from(slot.ts.clamp(1, 4)) - 1]))
                    })
                    .map(|(issi, _)| *issi)
                    .collect();
                for issi in gone {
                    state.pdch_by_issi.remove(&issi);
                    state.pdch_timeslots_by_issi.remove(&issi);
                }
            }
            taken
        };
        for slot in taken {
            let issis: Vec<u32> = self.pdch.iter().filter(|(_, p)| p.slots.contains(&slot)).map(|(i, _)| *i).collect();
            for issi in issis {
                let Some(p) = self.pdch.remove(&issi) else { continue };
                if p.slots.len() == 1 {
                    tracing::info!(
                        "SNDCP: PDCH ts {} of ISSI {} taken by a call, its data goes on on the MCCH",
                        ts_on(&p.slots, self.main_carrier),
                        issi
                    );
                    continue;
                }
                let mut state = config.state_write();
                for s in p.slots.iter().filter(|s| **s != slot) {
                    if let Err(e) = state.timeslot_alloc.release_slot(TimeslotOwner::PacketData, *s) {
                        tracing::debug!("SNDCP: PDCH ts {} of ISSI {} not ours any more: {:?}", s.ts, issi, e);
                    }
                }
                drop(state);
                tracing::info!(
                    "SNDCP: PDCH ts {} of ISSI {} taken by a call (ts {}), its data goes on on the MCCH",
                    ts_on(&p.slots, self.main_carrier),
                    issi,
                    slot.ts
                );
            }
        }
    }

    /// Each tick with `bearer = "pdch"`: the main-carrier slots in use are noted (see
    /// `PDCH_QUARANTINE_SLOTS`); the assignment going out puts the radio on its channel; an SN-END
    /// OF DATA that went out, or an assignment that never did or that the radio never
    /// acknowledged, ends it. A radio the LLC saw transmitting on the MCCH is off it.
    fn pdch_tick(&mut self, config: &SharedConfig) {
        if self.pdch_cfg.is_none() {
            return;
        }
        let main_carrier = self.main_carrier;
        let (off, back): (Vec<u32>, Vec<u32>) = {
            let state = config.state_read();
            for ts in 2..=4u8 {
                if state.timeslot_alloc.owner(ts).is_some() {
                    self.slot_busy_at[usize::from(ts)] = Some(self.clock);
                }
            }
            if let Some(c) = self.pdch_cfg.as_ref().and_then(|p| p.carrier.as_ref()) {
                for ts in 1..=4u8 {
                    if state.timeslot_alloc.slot_owner(CarrierSlot { carrier_num: c.num, ts }).is_some() {
                        self.carrier_busy_at[usize::from(ts)] = Some(self.clock);
                    }
                }
            }
            let off = self
                .pdch
                .iter()
                .filter(|(issi, p)| p.on_air && state.pdch_by_issi.get(issi).is_some_and(|g| !g.on_air))
                .map(|(issi, _)| *issi)
                .collect();
            // A radio of a channel of the packet-data carrier the LLC heard there again.
            let back = self
                .pdch
                .iter()
                .filter(|(issi, p)| {
                    !p.on_air
                        && p.assignment.is_none()
                        && p.slots.first().is_some_and(|s| s.carrier_num != main_carrier)
                        && state.pdch_by_issi.get(issi).is_some_and(|g| g.on_air)
                })
                .map(|(issi, _)| *issi)
                .collect();
            (off, back)
        };
        for issi in off {
            if let Some(carrier) = self.refused_its_carrier(config, issi) {
                self.stayed_on_the_mcch(config, issi, carrier);
            } else if let Some(p) = self.pdch.get_mut(&issi) {
                p.on_air = false;
                tracing::info!(
                    "SNDCP: ISSI {} transmitted on the MCCH, off its PDCH ts {} (kept for now)",
                    issi,
                    ts_on(&p.slots, main_carrier)
                );
            }
        }
        for issi in back {
            if let Some(p) = self.pdch.get_mut(&issi) {
                p.on_air = true;
                tracing::info!("SNDCP: ISSI {} is back on its PDCH ts {}", issi, ts_on(&p.slots, main_carrier));
            }
        }
        let mut on_air = Vec::new();
        let mut ended = Vec::new();
        for (issi, p) in self.pdch.iter_mut() {
            if let Some(r) = &p.assignment {
                match r.get_state() {
                    TxState::Pending => {}
                    state @ (TxState::Transmitted | TxState::Acknowledged) => {
                        p.assignment_ack = (state == TxState::Transmitted).then(|| r.clone());
                        p.assignment = None;
                        if !p.on_air {
                            p.on_air = true;
                            on_air.push(*issi);
                        }
                    }
                    TxState::Lost | TxState::Discarded => ended.push((*issi, "assignment not delivered")),
                }
            }
            match p.assignment_ack.as_ref().map(TxReporter::get_state) {
                Some(TxState::Acknowledged) => p.assignment_ack = None,
                Some(TxState::Lost) => ended.push((*issi, "the radio never acknowledged its assignment")),
                _ => {}
            }
            if p.quit.as_ref().is_some_and(|r| r.get_state() != TxState::Pending) {
                ended.push((*issi, "SN-END OF DATA, back to the MCCH"));
            }
        }
        on_air.sort_unstable();
        for issi in on_air {
            if let Some(p) = self.pdch.get(&issi) {
                tracing::info!("SNDCP: ISSI {} sent to its PDCH ts {}", issi, ts_on(&p.slots, self.main_carrier));
                Self::publish_pdch(config, issi, Some(p));
            }
        }
        ended.sort_unstable();
        for (issi, why) in ended {
            self.release_pdch(config, issi, why);
        }
    }

    /// SN-END OF DATA for `addr`: from its packet-data channel back to the MCCH when it is on one
    /// (the channel goes once this went out), else a plain one (and the channel goes now).
    fn end_of_data_msg(&mut self, config: &SharedConfig, addr: TetraAddress, link_id: u32, endpoint_id: u32, why: &str) -> Option<SapMsg> {
        let pdu = encode_end_of_data(&SndcpEndOfData {
            immediate_service_change: false,
        })
        .ok()?;
        let msg = tl_data_req(addr, link_id, endpoint_id, mle_sdu(&pdu));
        let main = self.pdch_cfg.as_ref().map(|p| p.main_carrier);
        match (self.pdch.get_mut(&addr.ssi), main) {
            (Some(p), Some(main)) if p.on_air => {
                let reporter = TxReporter::new();
                p.quit = Some(reporter.clone());
                tracing::info!(
                    "SNDCP: ISSI {} leaves its PDCH ts {} ({})",
                    addr.ssi,
                    ts_on(&p.slots, self.main_carrier),
                    why
                );
                Some(with_chan_alloc(msg, quit_to_mcch(main), &reporter))
            }
            _ => {
                self.release_pdch(config, addr.ssi, why);
                Some(msg)
            }
        }
    }

    fn on_transmit_request(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        bits: &str,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let full = self.multislot();
        let (nsapi, detail, capable) = match decode_data_transmit_request(&BitBuffer::from_bitstr(bits)) {
            Ok(r) => (
                r.nsapi,
                format!(
                    "link status {}, {}",
                    u8::from(r.logical_link_status),
                    resources(&r.resource_request, full)
                ),
                slots_capable(&r.resource_request),
            ),
            // The runtime needs only the NSAPI: an odd resource request still gets its answer.
            Err(e) => match field(bits, 4, 4).filter(|n| valid_nsapi(*n)) {
                Some(nsapi) => (nsapi, format!("rest not decoded: {e:?}"), 1),
                None => {
                    self.warn_malformed(issi, "SN-DATA TRANSMIT REQUEST");
                    return;
                }
            },
        };
        self.respond_transmit(queue, ind, nsapi, "SN-DATA TRANSMIT REQUEST", &detail, capable, wap, config, now);
    }

    /// Whether a radio's packet-data channel may have more than one slot here.
    fn multislot(&self) -> bool {
        self.pdch_cfg.as_ref().is_some_and(|p| p.max_slots > 1)
    }

    fn on_reconnect(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        bits: &str,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let full = self.multislot();
        // The NSAPI is there exactly when "data to send" is 1.
        let (nsapi, detail, capable) = match decode_reconnect(&BitBuffer::from_bitstr(bits)) {
            Ok(r) => (r.nsapi, resources(&r.resource_request, full), slots_capable(&r.resource_request)),
            Err(e) => match (field(bits, 4, 1), field(bits, 5, 4).filter(|n| valid_nsapi(*n))) {
                (Some(1), Some(nsapi)) => (Some(nsapi), format!("rest not decoded: {e:?}"), 1),
                (Some(0), _) => (None, format!("rest not decoded: {e:?}"), 1),
                _ => {
                    self.warn_malformed(issi, "SN-RECONNECT");
                    return;
                }
            },
        };
        match nsapi {
            Some(nsapi) => self.respond_transmit(queue, ind, nsapi, "SN-RECONNECT", &detail, capable, wap, config, now),
            None => self.reconnect_without_data(issi, &detail, wap, config, now),
        }
    }

    /// SN-RECONNECT with "data to send" = 0 (clause 28.3.4.2 c, table 28.18): the radio is in
    /// STANDBY and nothing goes back; its contexts enter STANDBY here too and its packet-data
    /// channel, which it left, is given back. Datagrams waiting for it stay held (the station does
    /// not send SN-DATA TRANSMIT REQUEST): they go down after its next request.
    fn reconnect_without_data(&mut self, issi: u32, detail: &str, wap: Option<&mut WapService>, config: &SharedConfig, now: Instant) {
        if !self.has_ctx(issi) {
            tracing::info!("SNDCP: SN-RECONNECT from ISSI {} without a PDP context, ignored", issi);
            return;
        }
        let (standby_slots, clock) = (self.standby_slots, self.clock);
        let mut held = 0;
        for (_, ctx) in self.ctxs.iter_mut().filter(|(k, _)| k.0 == issi) {
            ctx.state = CtxState::Standby;
            ctx.deadline = clock + standby_slots;
            held += ctx.dl.len();
        }
        tracing::info!(
            "SNDCP: SN-RECONNECT from ISSI {} without data to send ({}) -> STANDBY, no response{}",
            issi,
            detail,
            if held > 0 {
                format!(", {held} datagram(s) held until its next request")
            } else {
                String::new()
            }
        );
        self.release_pdch(config, issi, "SN-RECONNECT without data, the radio is on the MCCH");
        self.sync_pause(issi, wap, now);
    }

    /// SN-END OF DATA from a radio: STANDBY, answered with SN-END OF DATA; with "immediate service
    /// change" the radio has left already (clause 28.2.4.7 NOTE 3): no answer, and its packet-data
    /// channel goes now.
    fn on_end_of_data(
        &mut self,
        queue: &mut MessageQueue,
        ind: &LtpdMleUnitdataInd,
        bits: &str,
        wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let immediate = field(bits, 4, 1) == Some(1);
        let (standby_slots, clock) = (self.standby_slots, self.clock);
        for (key, ctx) in self.ctxs.iter_mut().filter(|(k, _)| k.0 == issi) {
            if ctx.state == CtxState::Ready {
                ctx.state = CtxState::Standby;
                ctx.deadline = clock + standby_slots;
                let c = ctx.counters;
                tracing::info!(
                    "SNDCP: SN-END OF DATA{} from ISSI {} NSAPI {} -> STANDBY (N-PDU up {} / down {}, {} / {} bytes)",
                    if immediate { " (immediate service change)" } else { "" },
                    issi,
                    key.1,
                    c.up,
                    c.down,
                    c.up_bytes,
                    c.down_bytes
                );
            }
        }
        if immediate {
            self.release_pdch(config, issi, "immediate service change, the radio left it");
            self.sync_pause(issi, wap, now);
            return;
        }
        if let Some(msg) = self.end_of_data_msg(
            config,
            ind.received_tetra_address,
            ind.link_id,
            ind.endpoint_id,
            "SN-END OF DATA from the radio",
        ) {
            queue.push_back(msg);
        }
        self.sync_pause(issi, wap, now);
    }

    /// A datagram from a radio: SN-UNITDATA (basic link) or SN-DATA (advanced link).
    fn on_user_data(
        &mut self,
        ind: &LtpdMleUnitdataInd,
        bits: &str,
        mut wap: Option<&mut WapService>,
        config: &SharedConfig,
        now: Instant,
    ) {
        let issi = ind.received_tetra_address.ssi;
        let (name, decoded) = if field(bits, 0, 4) == Some(SN_DATA) {
            ("SN-DATA", decode_sn_data_pdu(&BitBuffer::from_bitstr(bits)))
        } else {
            ("SN-UNITDATA", decode_sn_unitdata_pdu(&BitBuffer::from_bitstr(bits)))
        };
        let unitdata = match decoded {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!("SNDCP: {} from ISSI {} not usable ({:?}), dropped", name, issi, e);
                return;
            }
        };
        let key = (issi, unitdata.nsapi);
        let (ready_slots, clock) = (self.ready_slots, self.clock);
        // An SN-UNITDATA sent on the acknowledged basic link is answered as on the unacknowledged one.
        let bearer = match ind.bearer {
            LtpdBearer::BasicAck => LtpdBearer::BasicUnack,
            other => other,
        };
        let max_reply = self.max_reply_bytes(bearer);
        let Some(ctx) = self.ctxs.get_mut(&key) else {
            tracing::debug!(
                "SNDCP: {} from ISSI {} NSAPI {} without a PDP context, dropped",
                name,
                issi,
                unitdata.nsapi
            );
            return;
        };
        let Ok(npdu) = bitbuffer_npdu_octets(&unitdata.n_pdu) else {
            tracing::debug!("SNDCP: {} from ISSI {} is not whole octets, dropped", name, issi);
            return;
        };
        if ctx.state != CtxState::Ready {
            ctx.counters = Counters::default();
        }
        ctx.state = CtxState::Ready;
        ctx.deadline = clock + ready_slots;
        ctx.counters.up += 1;
        ctx.counters.up_bytes += npdu.len();
        ctx.reply_bearer = bearer;
        if let Some(p) = self.pdch.get_mut(&issi) {
            p.last_activity = clock;
        }
        // Anti-spoofing: a radio only sends from the address its context holds.
        let ctx_ip = ctx.ip;
        match parse_ipv4_packet(&npdu) {
            Ok(ip) if Ipv4Addr::from(ip.source) == ctx_ip => {}
            Ok(ip) => {
                tracing::debug!(
                    "SNDCP: ISSI {} sent from {} but holds {}, dropped",
                    issi,
                    Ipv4Addr::from(ip.source),
                    ctx_ip
                );
                return;
            }
            Err(e) => {
                tracing::debug!("SNDCP: ISSI {} sent an N-PDU that is not IPv4 ({:?}), dropped", issi, e);
                return;
            }
        }
        self.sync_pause(issi, wap.as_deref_mut(), now);
        let Some(wap) = wap else {
            tracing::debug!("SNDCP: datagram from ISSI {} dropped, [wap] is off", issi);
            return;
        };
        if let Err(e) = wap.on_air_ipv4(config, issi, &npdu, Some(max_reply), now) {
            tracing::debug!("SNDCP: datagram from ISSI {} not for the gateway ({:?}), dropped", issi, e);
        }
    }

    /// One stack tick: run the gateway, route its answers to the contexts, run the timers once a
    /// second and hand one datagram to the LLC when the MCCH is free of data.
    pub fn tick(&mut self, queue: &mut MessageQueue, config: &SharedConfig, mut wap: Option<&mut WapService>, now: Instant) {
        self.clock += 1;
        if let Some(wap) = wap.as_deref_mut() {
            for out in wap.tick(config, now) {
                self.queue_downlink(out);
            }
        }
        self.pdch_tick(config);
        if self.clock >= self.next_housekeeping {
            self.next_housekeeping = self.clock + HOUSEKEEPING_SLOTS;
            self.housekeeping(queue, config, wap.as_deref_mut(), now);
        }
        self.deliver(queue, config, wap, now);
    }

    fn next_ip_identification(&mut self) -> u16 {
        self.ip_identification = self.ip_identification.wrapping_add(1);
        self.ip_identification
    }

    fn queue_downlink(&mut self, out: UdpOut) {
        if out.peer.via != WapVia::Air {
            return;
        }
        let key = match self.by_ip.get(&out.peer.ip) {
            Some(key) if key.0 == out.peer.issi => *key,
            _ => {
                tracing::debug!(
                    "SNDCP: no PDP context of ISSI {} holds {}, reply dropped",
                    out.peer.issi,
                    out.peer.ip
                );
                return;
            }
        };
        let identification = self.next_ip_identification();
        let npdu = match out.to_ipv4(self.gateway, identification) {
            Ok(npdu) => npdu,
            Err(e) => {
                tracing::debug!("SNDCP: reply to ISSI {} not built ({:?})", out.peer.issi, e);
                return;
            }
        };
        let Some(ctx) = self.ctxs.get_mut(&key) else { return };
        ctx.dl.push_back(npdu);
        if ctx.dl.len() > MAX_DL_QUEUE {
            ctx.dl.pop_front();
            tracing::debug!("SNDCP: downlink queue of ISSI {} full, oldest datagram dropped", key.0);
        }
    }

    fn housekeeping(&mut self, queue: &mut MessageQueue, config: &SharedConfig, mut wap: Option<&mut WapService>, now: Instant) {
        self.drain_preempted(config);
        if self.ctxs.is_empty() {
            return;
        }
        let mut issis: Vec<u32> = self.ctxs.keys().map(|k| k.0).collect();
        issis.sort_unstable();
        issis.dedup();
        let status: HashMap<u32, (bool, bool)> = {
            let state = config.state_read();
            issis
                .iter()
                .map(|&issi| (issi, (state.subscribers.is_registered(issi), in_call(&state, issi))))
                .collect()
        };
        let (clock, standby_slots) = (self.clock, self.standby_slots);
        let mut released = Vec::new();
        let mut ended = Vec::new();
        for (key, ctx) in self.ctxs.iter_mut() {
            let (registered, call) = status.get(&key.0).copied().unwrap_or((false, false));
            if registered {
                ctx.seen_registered = true;
            } else if ctx.seen_registered {
                released.push((*key, "radio deregistered"));
                continue;
            }
            ctx.in_call = call;
            if clock < ctx.deadline {
                continue;
            }
            match ctx.state {
                CtxState::Ready => {
                    ctx.state = CtxState::Standby;
                    ctx.deadline = clock + standby_slots;
                    ended.push((*key, ctx.addr, ctx.counters, call));
                }
                CtxState::Standby => released.push((*key, "STANDBY expired")),
            }
        }
        ended.sort_unstable_by_key(|e| e.0);
        for (key, addr, c, call) in ended {
            // A radio in a call is on a traffic channel and would not hear it: STANDBY here only.
            tracing::info!(
                "SNDCP: READY expired for ISSI {} NSAPI {}: {}STANDBY (N-PDU up {} / down {}, {} / {} bytes)",
                key.0,
                key.1,
                if call {
                    "in a call, no SN-END OF DATA, "
                } else {
                    "SN-END OF DATA, "
                },
                c.up,
                c.down,
                c.up_bytes,
                c.down_bytes
            );
            if call {
                continue;
            }
            if let Some(msg) = self.end_of_data_msg(config, addr, 0, 0, "READY expired") {
                queue.push_back(msg);
            }
        }
        released.sort_unstable();
        let mut gone = Vec::new();
        for (key, why) in released {
            if let Some(ctx) = self.remove_ctx(key) {
                tracing::info!(
                    "SNDCP: PDP context of ISSI {} NSAPI {} ({}) released: {}",
                    key.0,
                    key.1,
                    ctx.ip,
                    why
                );
            }
            gone.push(key.0);
        }
        // One pass over the contexts per step, not one per radio: thousands of contexts must not
        // cost the stack thread a TDMA slot.
        if !gone.is_empty() {
            gone.dedup();
            let left: HashSet<u32> = self.ctxs.keys().map(|k| k.0).collect();
            for issi in gone.into_iter().filter(|issi| !left.contains(issi)) {
                self.forget_issi(issi, wap.as_deref_mut(), config);
            }
        }
        self.pdch_housekeeping(queue, config, &status, wap.as_deref());
        if let Some(wap) = wap {
            let mut reachable: HashMap<u32, bool> = HashMap::new();
            for (k, c) in &self.ctxs {
                *reachable.entry(k.0).or_default() |= c.state == CtxState::Ready && !c.in_call;
            }
            let mut reachable: Vec<(u32, bool)> = reachable.into_iter().collect();
            reachable.sort_unstable();
            for (issi, reachable) in reachable {
                self.set_paused(issi, !reachable, wap, now);
            }
        }
    }

    /// A packet-data channel goes when its radio went into a call (it listens to the call's
    /// channel), or after `pdch_idle_release_secs` without data and with no download running for
    /// it: a radio in READY is sent back to the MCCH with SN-END OF DATA and enters STANDBY
    /// (clause 28.2.6.2 NOTE 1: the SwMI releases the PDCH), else the slot just goes (the AACH
    /// shows it unallocated).
    fn pdch_housekeeping(
        &mut self,
        queue: &mut MessageQueue,
        config: &SharedConfig,
        status: &HashMap<u32, (bool, bool)>,
        wap: Option<&WapService>,
    ) {
        let Some(idle_slots) = self.pdch_cfg.as_ref().map(|p| p.idle_slots) else {
            return;
        };
        let clock = self.clock;
        let mut release: Vec<(u32, &str)> = Vec::new();
        let mut end: Vec<(u32, String)> = Vec::new();
        for (issi, p) in &self.pdch {
            if status.get(issi).is_some_and(|s| s.1) {
                release.push((*issi, "the radio is in a call"));
                continue;
            }
            let idle = p.assignment.is_none()
                && p.quit.is_none()
                && clock.saturating_sub(p.last_activity) >= idle_slots
                && !wap.is_some_and(|w| w.has_open_transactions(*issi))
                && !self.ctxs.iter().any(|(k, c)| k.0 == *issi && !c.dl.is_empty());
            if !idle {
                continue;
            }
            if self.ctxs.iter().any(|(k, c)| k.0 == *issi && c.state == CtxState::Ready) {
                end.push((*issi, ts_on(&p.slots, self.main_carrier)));
            } else {
                release.push((*issi, "idle"));
            }
        }
        release.sort_unstable();
        for (issi, why) in release {
            self.release_pdch(config, issi, why);
        }
        end.sort_unstable();
        let standby_slots = self.standby_slots;
        for (issi, ts) in end {
            let mut addr = None;
            for (_, ctx) in self.ctxs.iter_mut().filter(|(k, _)| k.0 == issi) {
                ctx.state = CtxState::Standby;
                ctx.deadline = clock + standby_slots;
                addr = Some(ctx.addr);
            }
            let Some(addr) = addr else { continue };
            tracing::info!(
                "SNDCP: PDCH ts {} of ISSI {} idle for {} s: SN-END OF DATA, STANDBY",
                ts,
                issi,
                idle_slots * 85 / 6 / 1000
            );
            if let Some(msg) = self.end_of_data_msg(config, addr, 0, 0, "idle") {
                queue.push_back(msg);
            }
        }
    }

    /// The next context in READY with something to send, round robin.
    fn next_to_serve(&self) -> Option<(u32, u8)> {
        let mut keys: Vec<(u32, u8)> = self
            .ctxs
            .iter()
            .filter(|(k, c)| c.state == CtxState::Ready && !c.in_call && !c.dl.is_empty() && !self.awaiting_ack.contains_key(&k.0))
            .map(|(k, _)| *k)
            .collect();
        keys.sort_unstable();
        let after = self.last_served;
        keys.iter().find(|k| after.is_none_or(|last| **k > last)).or(keys.first()).copied()
    }

    /// An SN-DATA the radio acknowledged restarts READY (clause 28.2.6.2).
    fn on_final(&mut self, inflight: &Inflight) {
        if inflight.reporter.get_state() == TxState::Acknowledged
            && let Some(ctx) = self.ctxs.get_mut(&inflight.key)
            && ctx.state == CtxState::Ready
        {
            ctx.deadline = self.clock + self.ready_slots;
        }
    }

    fn deliver(&mut self, queue: &mut MessageQueue, config: &SharedConfig, wap: Option<&mut WapService>, now: Instant) {
        // SN-DATA waiting for the radio's acknowledgement: acknowledged, lost or discarded now, or
        // not reported on for too long.
        if !self.awaiting_ack.is_empty() {
            let clock = self.clock;
            let mut done: Vec<u32> = self
                .awaiting_ack
                .iter()
                .filter(|(_, a)| a.reporter.is_in_final_state() || clock - a.since >= INFLIGHT_GUARD_SLOTS)
                .map(|(issi, _)| *issi)
                .collect();
            done.sort_unstable();
            for issi in done {
                if let Some(a) = self.awaiting_ack.remove(&issi) {
                    if !a.reporter.is_in_final_state() {
                        tracing::debug!(
                            "SNDCP: no acknowledgement report on the SN-DATA to ISSI {}, not waiting any longer",
                            issi
                        );
                    }
                    self.on_final(&a);
                }
            }
        }
        if let Some(inflight) = self.inflight.take() {
            // Out of the MAC: sent (basic link), all segments sent, acknowledged, lost or discarded
            // (advanced link). The acknowledgement of an SN-DATA is then awaited for its radio only.
            let state = inflight.reporter.get_state();
            if state == TxState::Pending && self.clock - inflight.since < INFLIGHT_GUARD_SLOTS {
                self.inflight = Some(inflight);
                return;
            }
            if state == TxState::Pending {
                tracing::debug!("SNDCP: no report on the last data PDU, not waiting for it any longer");
            } else if !inflight.reporter.is_in_final_state() {
                self.awaiting_ack.insert(inflight.key.0, inflight);
            } else {
                self.on_final(&inflight);
            }
        }
        let Some(key) = self.next_to_serve() else { return };
        // Voice first, on the live call map: a radio that has just gone into a call gets nothing.
        if in_call(&config.state_read(), key.0) {
            if let Some(ctx) = self.ctxs.get_mut(&key) {
                ctx.in_call = true;
            }
            self.sync_pause(key.0, wap, now);
            return;
        }
        let (ready_slots, clock) = (self.ready_slots, self.clock);
        let Some(ctx) = self.ctxs.get_mut(&key) else { return };
        let Some(npdu) = ctx.dl.pop_front() else { return };
        let bearer = ctx.reply_bearer;
        let al_number = match bearer {
            LtpdBearer::Advanced { al_number, .. } => Some(al_number),
            LtpdBearer::BasicAck | LtpdBearer::BasicUnack => None,
        };
        if al_number.is_none() && npdu.len() > BL_MAX_DATAGRAM {
            tracing::debug!(
                "SNDCP: datagram of {} bytes for ISSI {} does not fit the basic link, dropped",
                npdu.len(),
                key.0
            );
            return;
        }
        let n_pdu = BitBuffer::from_bytes(&npdu);
        let encoded = match al_number {
            Some(_) => encode_sn_data(key.1, SNDCP_NO_COMPRESSION, SNDCP_NO_COMPRESSION, &n_pdu),
            None => encode_sn_unitdata(key.1, SNDCP_NO_COMPRESSION, SNDCP_NO_COMPRESSION, &n_pdu),
        };
        let pdu = match encoded {
            Ok(pdu) => pdu,
            Err(e) => {
                tracing::debug!("SNDCP: datagram PDU for ISSI {} not built ({:?})", key.0, e);
                return;
            }
        };
        ctx.deadline = clock + ready_slots;
        ctx.counters.down += 1;
        ctx.counters.down_bytes += npdu.len();
        if let Some(p) = self.pdch.get_mut(&key.0) {
            p.last_activity = clock;
        }
        let (msg, reporter) = match al_number {
            Some(al_number) => {
                let reporter = TxReporter::new();
                tracing::debug!(
                    "SNDCP: -> ISSI {} SN-DATA on advanced link {}, {} bytes",
                    key.0,
                    al_number + 1,
                    npdu.len()
                );
                let msg = SapMsgInner::TlaTlDataReqAl(TlDataReqAl {
                    main_address: ctx.addr,
                    al_number,
                    tl_sdu: mle_sdu(&pdu),
                    tx_reporter: Some(reporter.clone()),
                });
                (msg, reporter)
            }
            None => {
                let reporter = TxReporter::new_unacked();
                tracing::debug!("SNDCP: -> ISSI {} SN-UNITDATA, {} bytes", key.0, npdu.len());
                let msg = SapMsgInner::TlaTlUnitdataReqBl(TlaTlUnitdataReqBl {
                    main_address: ctx.addr,
                    link_id: 0,
                    endpoint_id: 0,
                    tl_sdu: mle_sdu(&pdu),
                    stealing_permission: false,
                    subscriber_class: 0,
                    fcs_flag: false,
                    air_interface_encryption: None,
                    packet_data_flag: true,
                    n_tlsdu_repeats: 0,
                    data_class_info: None,
                    req_handle: 0,
                    chan_alloc: None,
                    tx_reporter: Some(reporter.clone()),
                });
                (msg, reporter)
            }
        };
        queue.push_back(SapMsg {
            sap: Sap::TlaSap,
            src: TetraEntity::Sndcp,
            dest: TetraEntity::Llc,
            msg,
        });
        self.inflight = Some(Inflight {
            key,
            reporter,
            since: clock,
        });
        self.last_served = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn slots_follow_the_timeslot_length() {
        assert_eq!(slots(85), 6);
        assert_eq!(slots(1_000), 71);
        assert_eq!(slots(28_000), 1977);
    }

    #[test]
    fn mtu_codes_match_table_28_79() {
        assert_eq!([296, 576, 1006, 1500, 2002].map(mtu_code), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn mle_sdu_prefixes_the_sndcp_discriminator() {
        let sdu = mle_sdu(&BitBuffer::from_bitstr("1011"));
        assert_eq!(sdu.to_bitstr(), "1001011");
    }

    const ISSI: u32 = 2_260_618;

    fn shared_config() -> SharedConfig {
        let toml = r#"
config_version = "0.6"
stack_mode = "Bs"

[phy_io]
backend = "None"

[net_info]
mcc = 901
mnc = 9999

[cell_info]
main_carrier = 1584
freq_band = 4
freq_offset = 0
duplex_spacing = 4
reverse_operation = false
location_area = 1

[wap]
enabled = true

[packet_data]
enabled = true
"#;
        SharedConfig::from_parts(tetra_config::bluestation::from_toml_str(toml).unwrap(), None)
    }

    fn ind(sn: &str) -> LtpdMleUnitdataInd {
        LtpdMleUnitdataInd {
            sdu: BitBuffer::from_bitstr(&format!("100{sn}")),
            endpoint_id: 0,
            link_id: 1,
            received_tetra_address: TetraAddress::issi(ISSI),
            chan_change_resp_req: false,
            chan_change_handle: None,
            bearer: tetra_saps::ltpd::LtpdBearer::BasicAck,
        }
    }

    /// Runtime with one context of ISSI on NSAPI 1 (10.0.0.2) in READY.
    fn ready_runtime(config: &SharedConfig, wap: &mut WapService, now: Instant) -> PacketDataRuntime {
        let mut rt = PacketDataRuntime::new(config).expect("[packet_data] enabled");
        let mut queue = MessageQueue::new();
        rt.rx(&mut queue, &ind("0000000100010010001000000000"), Some(wap), config, now);
        rt.rx(&mut queue, &ind("01100001000"), Some(wap), config, now);
        assert_eq!(
            rt.ctxs.get(&(ISSI, 1)).map(|c| (c.ip, c.state)),
            Some((Ipv4Addr::new(10, 0, 0, 2), CtxState::Ready))
        );
        rt
    }

    fn get_status_page(rt: &mut PacketDataRuntime, config: &SharedConfig, wap: &mut WapService, now: Instant) {
        let mut wtp = vec![0x0a, 0x00, 0x51, 0x12, 0x40, 11];
        wtp.extend_from_slice(b"/status.wml");
        let npdu = super::super::ip::build_ipv4_udp_npdu([10, 0, 0, 2], [10, 0, 0, 1], 2049, 9201, &wtp, 1, 64).unwrap();
        let pdu = encode_sn_unitdata(1, 0, 0, &BitBuffer::from_bytes(&npdu)).unwrap();
        let mut sn = ind(&pdu.to_bitstr());
        sn.bearer = tetra_saps::ltpd::LtpdBearer::BasicUnack;
        rt.rx(&mut MessageQueue::new(), &sn, Some(wap), config, now);
    }

    fn unitdata_reqs(queue: &mut MessageQueue) -> usize {
        let mut n = 0;
        while let Some(msg) = queue.pop_front() {
            if matches!(msg.msg, SapMsgInner::TlaTlUnitdataReqBl(_)) {
                n += 1;
            }
        }
        n
    }

    /// A radio in a call gets nothing, and the gateway's retransmission timers wait for it: a
    /// minute later the result is still the only datagram queued (no retransmission, no Abort),
    /// and it goes down once the call is over.
    #[test]
    fn a_call_holds_the_data_and_pauses_the_gateway_timers() {
        let config = shared_config();
        let mut wap = WapService::start(&config).unwrap();
        let t0 = Instant::now();
        let mut rt = ready_runtime(&config, &mut wap, t0);
        config.state_write().active_call_ts.insert(ISSI, (1584, 2, 4));
        get_status_page(&mut rt, &config, &mut wap, t0);
        let mut queue = MessageQueue::new();
        rt.tick(&mut queue, &config, Some(&mut wap), t0);
        assert_eq!(unitdata_reqs(&mut queue), 0, "radio in a call");
        assert!(rt.paused.contains(&ISSI));
        for i in 0..HOUSEKEEPING_SLOTS {
            rt.tick(
                &mut queue,
                &config,
                Some(&mut wap),
                t0 + Duration::from_secs(60) + Duration::from_millis(i),
            );
        }
        assert_eq!(unitdata_reqs(&mut queue), 0);
        assert_eq!(rt.ctxs[&(ISSI, 1)].dl.len(), 1, "no retransmission and no Abort while paused");
        config.state_write().active_call_ts.clear();
        let t1 = t0 + Duration::from_secs(61);
        for _ in 0..HOUSEKEEPING_SLOTS {
            rt.tick(&mut queue, &config, Some(&mut wap), t1);
        }
        assert!(!rt.paused.contains(&ISSI));
        assert_eq!(unitdata_reqs(&mut queue), 1, "the result goes down after the call");
        assert!(wap.has_open_transactions(ISSI), "the transaction survived the call");
    }

    /// READY expiry: SN-END OF DATA, STANDBY and the gateway paused; STANDBY expiry: the context and
    /// its address are released.
    #[test]
    fn ready_then_standby_expiry() {
        let config = shared_config();
        let mut wap = WapService::start(&config).unwrap();
        let now = Instant::now();
        let mut rt = ready_runtime(&config, &mut wap, now);
        let mut queue = MessageQueue::new();
        let deadline = rt.ctxs[&(ISSI, 1)].deadline;
        rt.clock = deadline - 1;
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        assert!(queue.pop_front().is_none(), "READY still running");
        rt.clock = deadline;
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        let Some(SapMsgInner::TlaTlDataReqBl(eod)) = queue.pop_front().map(|m| m.msg) else {
            panic!("SN-END OF DATA expected");
        };
        assert_eq!(
            eod.tl_sdu.to_bitstr(),
            "100100000",
            "discriminator, END OF DATA, no service change, o-bit"
        );
        assert_eq!(rt.ctxs[&(ISSI, 1)].state, CtxState::Standby);
        assert!(rt.paused.contains(&ISSI));
        let standby_deadline = rt.ctxs[&(ISSI, 1)].deadline;
        assert_eq!(standby_deadline - deadline, rt.standby_slots);
        rt.clock = standby_deadline - 1;
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        assert!(rt.ctxs.contains_key(&(ISSI, 1)));
        rt.clock = standby_deadline;
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        assert!(rt.ctxs.is_empty() && rt.by_ip.is_empty() && rt.paused.is_empty());
        assert_eq!(rt.allocate(), Some(Ipv4Addr::new(10, 0, 0, 2)), "address back in the pool");
    }

    /// Many radios: one housekeeping pauses the gateway for each radio in STANDBY and, once their
    /// STANDBY ends, forgets them all.
    #[test]
    fn housekeeping_of_many_radios() {
        let config = shared_config();
        let mut wap = WapService::start(&config).unwrap();
        let now = Instant::now();
        let mut rt = PacketDataRuntime::new(&config).expect("[packet_data] enabled");
        let mut queue = MessageQueue::new();
        for i in 0..200 {
            let mut demand = ind("0000000100010010001000000000");
            demand.received_tetra_address = TetraAddress::issi(ISSI + i);
            rt.rx(&mut queue, &demand, Some(&mut wap), &config, now);
        }
        assert_eq!(rt.ctxs.len(), 200);
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        assert_eq!(rt.paused.len(), 200, "all in STANDBY");
        rt.clock += rt.standby_slots;
        rt.housekeeping(&mut queue, &config, Some(&mut wap), now);
        assert!(rt.ctxs.is_empty() && rt.by_ip.is_empty() && rt.paused.is_empty());
    }

    /// SN-RECONNECT without data to send (table 28.18): no answer, the context enters STANDBY and
    /// the gateway waits.
    #[test]
    fn a_reconnect_without_data_is_not_answered_and_enters_standby() {
        use super::super::transfer::{SndcpReconnect, encode_reconnect};
        let config = shared_config();
        let mut wap = WapService::start(&config).unwrap();
        let now = Instant::now();
        let mut rt = ready_runtime(&config, &mut wap, now);
        let pdu = encode_reconnect(&SndcpReconnect {
            nsapi: None,
            resource_request: SndcpPacketDataResourceRequest::None,
        })
        .unwrap();
        let mut queue = MessageQueue::new();
        rt.rx(&mut queue, &ind(&pdu.to_bitstr()), Some(&mut wap), &config, now);
        assert!(queue.pop_front().is_none(), "no response");
        assert_eq!(rt.ctxs[&(ISSI, 1)].state, CtxState::Standby);
        assert!(rt.paused.contains(&ISSI));
    }
}
