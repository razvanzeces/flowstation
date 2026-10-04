use tetra_core::{
    BitBuffer, Direction, LinkId, PhyBlockNum, PhysicalChannel, SsiType, TdmaTime, TetraAddress, Todo, TxReporter, unimplemented_log,
};
use tetra_saps::{
    control::call_control::{Circuit, CircuitDlMediaSource},
    tmv::{TmvUnitdataReq, TmvUnitdataReqSlot, enums::logical_chans::LogicalChannel},
};

use tetra_pdus::{
    mle::pdus::{d_mle_sync::DMleSync, d_mle_sysinfo::DMleSysinfo},
    umac::{
        enums::{
            access_assign_dl_usage::AccessAssignDlUsage, access_assign_ul_usage::AccessAssignUlUsage,
            basic_slotgrant_cap_alloc::BasicSlotgrantCapAlloc, basic_slotgrant_granting_delay::BasicSlotgrantGrantingDelay,
            reservation_requirement::ReservationRequirement,
        },
        fields::basic_slotgrant::BasicSlotgrant,
        pdus::{
            access_assign::{AccessAssign, AccessField},
            access_assign_fr18::AccessAssignFr18,
            mac_resource::MacResource,
            mac_sync::MacSync,
            mac_sysinfo::MacSysinfo,
        },
    },
};

use crate::{
    lmac::components::scrambler,
    umac::subcomp::{
        bs_frag::{BsFragger, MIN_SLOT_CAP_FOR_FRAG, MIN_SLOT_CAP_FOR_RES_FRAG_START},
        circuit_mgr::CircuitMgr,
    },
};

/// Packet-data channels of several slots: the half-duplex guard and their uplink grants.
mod multislot;

/// We submit this many TX timeslots ahead of the current time
pub const MACSCHED_TX_AHEAD: usize = 1;

// We schedule up to this many frames ahead
pub const MACSCHED_NUM_FRAMES: usize = 18;

/// Largest granting delay representable in the 4-bit basic slot granting delay field
/// (EN 300 392-2 cl. 21.5.6); 14 and 15 are reserved codes.
pub const MAX_GRANTING_DELAY_OPPORTUNITIES: usize = 13;

/// Largest capacity one slot grant hands a radio on its packet-data channel. A larger reservation
/// requirement is granted in further chunks as the radio uses them: the BS may use more than one
/// slot grant, and the slots need not be continuous (EN 300 392-2 23.4.2.1.2 NOTE 9). An MXP600
/// asks for 51 slots at once for a WSP Connect over the advanced link, which no single grant in
/// the schedule window can hold; Nexus-BS grants such requests 4 slots at a time.
const PDCH_UL_GRANT_CHUNK_SLOTS: usize = 4;

const NULL_PDU_LEN_BITS: usize = 16;

pub const SCH_HD_CAP: usize = 124;
pub const SCH_F_CAP: usize = 268;
pub const TCH_S_CAP: usize = 274;

/// Number of timeslots the scheduler operates on. May become larger when secondary carriers are supported.
pub const NUM_TIMESLOTS: usize = 4;

/// A slot of a carrier without MCCH that stops being a packet-data channel sends SCH/F with the
/// AACH unallocated for this long (one multiframe) instead of BSCH + BNCH, whose AACH a radio on
/// an assigned channel does not read (EN 300 392-2 9.5.1b NOTE): a radio still there leaves after
/// N.208 adverse AACHs (23.5.6.1.1) rather than T.208.
pub const PDCH_RELEASED_UNALLOCATED_SLOTS: i32 = 72;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarrierDownlinkMode {
    PrimaryMcch,
    SecondaryBcchNoMcch,
    TrafficOnly,
}

impl CarrierDownlinkMode {
    fn emit_bcch(self) -> bool {
        matches!(self, Self::PrimaryMcch | Self::SecondaryBcchNoMcch)
    }

    fn allow_mcch(self) -> bool {
        matches!(self, Self::PrimaryMcch)
    }

    fn allow_common_control_aach(self) -> bool {
        matches!(self, Self::PrimaryMcch)
    }

    fn allow_assigned_traffic_ts1(self) -> bool {
        matches!(self, Self::SecondaryBcchNoMcch | Self::TrafficOnly)
    }
}

#[derive(Clone, Debug)]
pub struct PrecomputedUmacPdus {
    pub mac_sysinfo1: MacSysinfo,
    pub mac_sysinfo2: MacSysinfo,
    pub mle_sysinfo: DMleSysinfo,
    pub mac_sync: MacSync,
    pub mle_sync: DMleSync,
}

#[derive(Debug)]
pub struct TimeslotSchedule {
    pub ul1: Option<u32>,
    pub ul2: Option<u32>,
    /// Usage marker (4-62) issued to an MS that received a multi-slot grant.
    /// When set, AACH for this slot signals `Traffic(marker)` so the MS knows
    /// the slot is reserved for it. The marker remains until both ul1 and ul2
    /// are consumed/freed.
    ///
    /// Per ETSI TS 100 392-2 §23.5.1: usage markers 0 (= unallocated) and
    /// 1-3 are reserved. 63 (= common linearisation channel) is reserved.
    /// Valid range for BS-assigned reservations is 4..=62.
    pub usage_marker: Option<u8>,
    // pub dl: Option<TmvUnitdataReq>,
}

// #[derive(Debug)]
pub struct BsChannelScheduler {
    pub cur_dltime: TdmaTime,
    carrier_num: u16,
    downlink_mode: CarrierDownlinkMode,
    scrambling_code: u32,
    precomps: PrecomputedUmacPdus,
    /// Collect dltx traffic here that can't be sent this slot.
    /// Swapped back into the dltx_queues method at the end of the tick.
    dltx_next_slot_queue: Vec<DlSchedElem>,
    /// Four queues for scheduled downlink traffic, one per timeslot
    dltx_queues: [Vec<DlSchedElem>; 4],
    ulsched: [[TimeslotSchedule; MACSCHED_NUM_FRAMES]; 4],

    circuits: CircuitMgr,

    /// When true, the given timeslot is in call hangtime: keep circuit allocated but stop
    /// sending traffic-plane TCH blocks. Instead, transmit signalling-plane idle (Null PDUs)
    /// and signal UL usage as AssignedOnly so MS can request the floor.
    hangtime: [bool; 4],

    /// True if a MAC-RESOURCE PDU with a chan_alloc element has already been enqueued for ts1
    /// in the current frame. The second such PDU (e.g. DConnectAck MCCH) must be deferred to
    /// the next frame to avoid exceeding the 216-bit slot capacity (DConnect+DConnectAck=223 bits).
    mcch_chan_alloc_sent_this_frame: bool,

    /// Per-timeslot rotating cursor for allocating usage markers to multi-slot
    /// uplink reservations. Wraps in the valid range [4, 62] (0 = unallocated,
    /// 1-3 reserved, 63 = common linearisation; per ETSI TS 100 392-2 §23.5.1).
    ///
    /// A multi-slot grant without a usage_marker leaves the MS unable to
    /// associate AACH slot signalling with its own reservation — empirically
    /// MS-side stacks (MXP600 etc.) abandon the burst after the first slot and
    /// fall back to repeated random access, which never completes a
    /// fragmented MM PDU (e.g. ULocationUpdate when re-entering coverage).
    /// Issuing a real marker fixes that.
    next_usage_marker: [u8; 4],

    /// Packet-data channel per timeslot: the ISSI it is assigned to (`[packet_data] bearer =
    /// "pdch"`: main carrier ts 2..=4, packet-data carrier ts 1..=4). The UMAC sets it from the
    /// shared state every tick.
    pdch: [Option<u32>; 4],

    /// When each slot stopped being a packet-data channel, and whose it was. For a multiframe
    /// after that the previous owner's uplink there is not taken for an adjacent-channel copy,
    /// and on a carrier without MCCH the slot sends SCH/F with the AACH unallocated (a radio still
    /// there leaves, see `PDCH_RELEASED_UNALLOCATED_SLOTS`).
    pdch_released_at: [Option<TdmaTime>; 4],
    pdch_released_owner: [Option<u32>; 4],

    /// Main carrier only, with a packet-data carrier in use: that carrier. The packet-data channel
    /// assignments to it built here are listed for the UMAC (`take_assignments_elsewhere`), and no
    /// grant rides on them.
    pdch_handoff_carrier: Option<u16>,
    /// The packet-data channels of `pdch_handoff_carrier` (owner per timeslot), as the UMAC last
    /// set them: what tells a packet-data channel assignment to that carrier from a call's
    /// channel allocation there (see `is_handoff_assignment`).
    pdch_handoff_owners: [Option<u32>; 4],
    /// Channel assignments to `pdch_handoff_carrier` that went out: (ISSI, timeslots, downlink
    /// slot that carried it).
    pdch_assignments_elsewhere: Vec<(u32, [bool; 4], TdmaTime)>,
    /// A radio sent here from another carrier's MCCH, by its channel's queue: the first downlink
    /// slot and the first uplink slot of its channel (EN 300 392-2 23.5.4.3.1 rule 1 a), see
    /// `multislot`.
    pdch_arrival: [Option<(TdmaTime, TdmaTime)>; 4],

    /// Uplink capacity still owed on a packet-data channel after a chunked grant: the radio, the
    /// slots left and the last slot of the chunk already granted.
    pdch_ul_debt: [Option<(TetraAddress, usize, TdmaTime)>; 4],

    /// Downlink slot that carried the channel assignment of a packet-data channel of several
    /// slots, by the channel's lowest slot: the slot right after it is not received on the new
    /// channel (see `multislot`).
    pdch_assigned_at: [Option<TdmaTime>; 4],

    /// Uplink capacity owed to the radio of a packet-data channel of several slots, by the
    /// channel's lowest slot, granted when a slot of the channel it hears is built (see
    /// `multislot`). Never in `pdch_ul_debt`, which keeps its one-slot meaning.
    pdch_channel_debt: [Option<multislot::ChannelDebt>; 4],

    /// The grant `multislot` placed in the slot being built, checked once it is built.
    pdch_lazy: Option<multislot::LazyGrant>,

    /// Grants `multislot` placed that left, by the downlink slot that carried them: the radio and
    /// the uplink slots they reserved (the last few slots only; see `pdch_random_access`).
    pdch_grants_out: Vec<(TdmaTime, u32, Vec<TdmaTime>)>,

    /// Queued advanced link segments to the radio of a packet-data channel of several slots that
    /// ask for an acknowledgement, by the radio and the segment's report: each gets a slot for the
    /// answer granted with it (see `multislot`).
    pdch_reply_wanted: Vec<(u32, TxReporter)>,

    /// `secondary_carrier_on_demand`: carriers not on the air long enough yet (off or warming up).
    /// A channel allocation sending a radio to one of them waits (see `dl_take_next_sched_item`).
    /// The UMAC sets it every tick; always empty without the feature.
    cold_carriers: Vec<u16>,
}

#[derive(Debug)]
pub enum DlSchedElem {
    /// A SYSINFO or neighboring cells info block. The integer determines which of the precomputed blocks to use (SYSINFO1, SYSINFO2, NEIGHBORING_CELLS
    Broadcast(Todo),

    /// A received MAC-ACCESS PDU still has to be acknowledged
    RandomAccessAck(TetraAddress),

    /// A slotgrant response, which has to be transmitted with high priority or the delay numbers will be off.
    /// ssi, BasicSlotgrant, and an optional usage_marker are provided. When the grant covers >1 slot the
    /// scheduler allocates a usage marker so AACH and the MacResource ACK can identify the reservation
    /// (per ETSI TS 100 392-2 §21.4.3.2 and §23.5.1); single-slot grants don't need one.
    Grant(TetraAddress, BasicSlotgrant, Option<u8>),

    /// A MAC-RESOURCE PDU. May be split into fragments upon processing, in which case a FragBuf will be inserted after processing the resource.
    /// The flag marks a packet-data channel assignment (see `dl_enqueue_pdch_assignment_for_link`).
    Resource(MacResource, BitBuffer, Option<TxReporter>, bool),

    /// A FragBuf containing remaining non-transmitted information after a MAC-RESOURCE start has been transmitted
    FragBuf(BsFragger),

    /// Pre-built STCH block for FACCH/stealing a half-slot from traffic channel.
    /// Contains MAC-U-SIGNAL (3 bits) + TM-SDU = 124 type1 bits.
    /// Delivers time-critical signaling (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
    /// The third field, when present, is a second STCH block that steals the second half of the
    /// SAME slot: a MAC-RESOURCE with LI=111111 in the first half and its MAC-END in the second,
    /// the only fragmentation TS 100 392-2 23.4.2.1.7 allows on stolen slots.
    /// The fourth field is set when the PDU is for the MS transmitting on the slot: it listens
    /// only to the frames of its monitoring pattern (9.6), so the block waits for one of them.
    Stealing(BitBuffer, Option<TxReporter>, Option<BitBuffer>, bool),
}

const EMPTY_SCHED_ELEM: TimeslotSchedule = TimeslotSchedule {
    ul1: None,
    ul2: None,
    usage_marker: None,
    // dl: None,
};
const EMPTY_SCHED_CHANNEL: [TimeslotSchedule; MACSCHED_NUM_FRAMES] = [EMPTY_SCHED_ELEM; MACSCHED_NUM_FRAMES];
const EMPTY_SCHED: [[TimeslotSchedule; MACSCHED_NUM_FRAMES]; 4] = [EMPTY_SCHED_CHANNEL; 4];

impl BsChannelScheduler {
    pub fn new(scrambling_code: u32, precomps: PrecomputedUmacPdus) -> Self {
        let carrier_num = precomps.mac_sysinfo1.main_carrier;
        BsChannelScheduler {
            cur_dltime: TdmaTime { t: 0, f: 0, m: 0, h: 0 }, // Intentionally invalid, updated in tick function
            carrier_num,
            downlink_mode: CarrierDownlinkMode::PrimaryMcch,
            scrambling_code,
            precomps,
            dltx_next_slot_queue: Vec::new(),
            dltx_queues: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            ulsched: EMPTY_SCHED,
            circuits: CircuitMgr::new(),
            hangtime: [false, false, false, false],
            mcch_chan_alloc_sent_this_frame: false,
            // Start each timeslot's marker cursor at 4 (first valid value).
            next_usage_marker: [4, 4, 4, 4],
            pdch: [None; 4],
            pdch_released_at: [None; 4],
            pdch_released_owner: [None; 4],
            pdch_handoff_carrier: None,
            pdch_handoff_owners: [None; 4],
            pdch_assignments_elsewhere: Vec::new(),
            pdch_arrival: [None; 4],
            pdch_ul_debt: [None; 4],
            pdch_assigned_at: [None; 4],
            pdch_channel_debt: [None; 4],
            pdch_lazy: None,
            pdch_grants_out: Vec::new(),
            pdch_reply_wanted: Vec::new(),
            cold_carriers: Vec::new(),
        }
    }

    pub fn set_carrier_num(&mut self, carrier_num: u16) {
        self.carrier_num = carrier_num;
    }

    pub fn set_downlink_mode(&mut self, downlink_mode: CarrierDownlinkMode) {
        self.downlink_mode = downlink_mode;
    }

    pub fn carrier_num(&self) -> u16 {
        self.carrier_num
    }

    /// The carriers a channel allocation must not send a radio to yet (`secondary_carrier_on_demand`).
    pub fn set_cold_carriers(&mut self, carriers: &[u16]) {
        self.cold_carriers.clear();
        self.cold_carriers.extend_from_slice(carriers);
    }

    /// The carrier a channel allocation queued as `e` sends its radio to (a quit sends it nowhere).
    fn allocation_carrier(e: &DlSchedElem) -> Option<u16> {
        let a = match e {
            DlSchedElem::Resource(pdu, ..) => pdu.chan_alloc_element.as_ref(),
            DlSchedElem::FragBuf(f) if !f.is_started() => f.chan_alloc(),
            _ => None,
        }?;
        a.ts_assigned.iter().any(|s| *s).then_some(a.carrier_num)
    }

    /// Carriers the channel allocations queued here send radios to.
    pub fn allocation_carriers(&self) -> impl Iterator<Item = u16> + '_ {
        self.dltx_queues
            .iter()
            .flatten()
            .chain(&self.dltx_next_slot_queue)
            .filter_map(Self::allocation_carrier)
    }

    pub fn allow_mcch(&self) -> bool {
        self.downlink_mode.allow_mcch()
    }

    pub fn allow_common_control_aach(&self) -> bool {
        self.downlink_mode.allow_common_control_aach()
    }

    /// Drop the downlink voice still queued on `ts`: a local speaker was just given the floor, so
    /// nothing in the queue can be theirs yet. Up to MAX_QUEUED_DL_BLOCKS (240 ms) of the previous
    /// turn used to be left behind at hangtime and played at the start of the next turn, right
    /// after the D-TX GRANTED that named the new speaker.
    pub fn drop_queued_voice(&mut self, ts: u8) {
        let dropped = self.circuits.drop_queued_blocks(self.carrier_num, ts);
        if dropped > 0 {
            tracing::debug!(
                "BsChannelScheduler: dropped {} queued DL voice blocks of the previous turn on carrier={} ts {}",
                dropped,
                self.carrier_num,
                ts
            );
        }
    }

    /// Enter/leave hangtime for an assigned traffic timeslot.
    pub fn set_hangtime(&mut self, ts: u8, active: bool) {
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::set_hangtime: invalid ts {}", ts);
            return;
        }

        let idx = ts as usize - 1;
        self.hangtime[idx] = active;

        // When leaving hangtime, drain stale signaling items that can only be consumed
        // in signaling mode. Keep Stealing items — they carry D-TX GRANTED/CEASED
        // that still need FACCH delivery.
        if !active {
            self.dl_drop_all_except_stolen(ts);
        }

        tracing::info!(
            "BsChannelScheduler: hangtime {} for ts {}",
            if active { "ENABLED" } else { "DISABLED" },
            ts,
        );
    }

    pub fn is_hangtime(&self, ts: u8) -> bool {
        // Defensive bounds check: ts must be 1..=4. Without this, a caller
        // accidentally passing ts=0 would underflow `ts as usize - 1` to
        // usize::MAX and panic on the array index. set_hangtime already has
        // this guard; mirror it here. Credit to proxiboi69 in
        // MidnightBlueLabs/tetra-bluestation PR #85.
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::is_hangtime: invalid ts {}", ts);
            return false;
        }
        self.hangtime[ts as usize - 1]
    }

    fn is_hangtime_effective(&self, ts: u8) -> bool {
        if !(1..=4).contains(&ts) {
            tracing::warn!("BsChannelScheduler::is_hangtime_effective: invalid ts {}", ts);
            return false;
        }
        let idx = ts as usize - 1;
        if !self.hangtime[idx] {
            return false;
        }
        // If a stealing block is still queued for this slot, keep traffic mode
        // so it can be delivered via FACCH.
        !self.has_pending_stealing(ts)
    }

    pub fn has_pending_stealing(&self, ts: u8) -> bool {
        let slot = ts as usize - 1;
        self.dltx_queues
            .get(slot)
            .map(|q| q.iter().any(|e| matches!(e, DlSchedElem::Stealing(..))))
            .unwrap_or(false)
    }

    /// Whether `ts` is an ASSIGNED TRAFFIC slot on this carrier: ts 2..=4 always; ts1 only when
    /// this carrier has no MCCH on it (a secondary carrier). Circuit closes on such slots are
    /// deferred by the UMAC so queued FACCH/STCH signalling can leave first.
    pub fn supports_assigned_traffic_ts(&self, ts: u8) -> bool {
        match ts {
            1 => self.downlink_mode.allow_assigned_traffic_ts1(),
            2..=4 => true,
            _ => false,
        }
    }

    /// Make `ts` (2..=4, and 1 on a carrier without MCCH) the packet-data channel of `owner`, or
    /// no packet-data channel. When it stops being one radio's PDCH, the signalling still queued
    /// there for that radio is dropped: the radio goes back to the MCCH on its own when the AACH
    /// no longer shows assigned control (EN 300 392-2 23.5.6.1.1), and the LLC sends again what
    /// it must.
    pub fn set_pdch(&mut self, ts: u8, owner: Option<u32>) {
        if !self.supports_assigned_traffic_ts(ts) {
            return;
        }
        let idx = ts as usize - 1;
        let old = self.pdch[idx];
        if old == owner {
            return;
        }
        if let Some(old) = old {
            self.pdch_note_uplink_left_behind(old);
            self.pdch_reply_wanted.retain(|(ssi, _)| *ssi != old);
        }
        self.pdch[idx] = owner;
        self.pdch_ul_debt[idx] = None;
        self.pdch_assigned_at[idx] = None;
        self.pdch_channel_debt[idx] = None;
        self.pdch_arrival[idx] = None;
        // On a carrier without MCCH the slot then shows itself unallocated for a while (9.5.1b).
        let other_carrier = !self.allow_mcch();
        self.pdch_released_at[idx] = old.filter(|_| owner.is_none()).map(|_| self.cur_dltime);
        self.pdch_released_owner[idx] = old.filter(|_| owner.is_none());
        let of_carrier = if other_carrier {
            format!(" of carrier {}", self.carrier_num)
        } else {
            String::new()
        };
        if let Some(old) = old {
            let dropped = self.dl_drop_queued_for_ssi(ts, old);
            tracing::info!(
                "UMAC: ts {}{} is no longer the PDCH of ISSI {} ({}; {} queued PDU(s) dropped)",
                ts,
                of_carrier,
                old,
                if self.circuits.is_active(Direction::Dl, self.carrier_num, ts) {
                    "taken by a call"
                } else {
                    "released"
                },
                dropped
            );
        }
        if let Some(issi) = owner {
            tracing::info!("UMAC: ts {}{} is the PDCH of ISSI {} (AACH assigned control)", ts, of_carrier, issi);
        }
    }

    /// Whether this scheduler holds a packet-data channel.
    pub fn has_pdch(&self) -> bool {
        self.pdch.iter().any(Option::is_some)
    }

    /// The radio whose packet-data channel `ts` stopped being less than
    /// `PDCH_RELEASED_UNALLOCATED_SLOTS` ago (its last bursts there are its own).
    pub fn pdch_released_owner(&self, ts: u8) -> Option<u32> {
        let idx = usize::from(ts).checked_sub(1).filter(|i| *i < 4)?;
        let at = self.pdch_released_at[idx]?;
        (0..=PDCH_RELEASED_UNALLOCATED_SLOTS)
            .contains(&self.cur_dltime.diff(at))
            .then_some(self.pdch_released_owner[idx])
            .flatten()
    }

    /// Whether downlink slot `ts` stopped being a packet-data channel of this carrier without
    /// MCCH less than `PDCH_RELEASED_UNALLOCATED_SLOTS` ago.
    fn pdch_recently_released(&self, ts: TdmaTime) -> bool {
        self.pdch_released_at[ts.t as usize - 1].is_some_and(|at| (0..=PDCH_RELEASED_UNALLOCATED_SLOTS).contains(&ts.diff(at)))
    }

    /// Main carrier, with a packet-data carrier in use: `carrier` (see `pdch_handoff_carrier`).
    pub fn set_pdch_handoff_carrier(&mut self, carrier: Option<u16>) {
        self.pdch_handoff_carrier = carrier;
    }

    /// Main carrier, with a packet-data carrier in use: the packet-data channels of that carrier
    /// (owner per timeslot), set by the UMAC every tick before the slot is built.
    pub fn set_pdch_handoff_owners(&mut self, owners: [Option<u32>; 4]) {
        self.pdch_handoff_owners = owners;
    }

    /// Whether `pdu` assigns its radio its packet-data channel of the packet-data carrier
    /// `carrier` (whose channels are `owners`): an individual channel allocation to that carrier
    /// whose timeslots are that radio's channel there. miura does not mark a packet-data channel
    /// assignment in the queue; a call's channel allocation to that carrier is never a radio's
    /// channel there (a call that takes a slot of one takes the whole channel).
    fn is_handoff_assignment(pdu: &MacResource, carrier: Option<u16>, owners: [Option<u32>; 4]) -> bool {
        let (Some(c), Some(addr)) = (pdu.chan_alloc_element.as_ref(), pdu.addr) else {
            return false;
        };
        addr.ssi_type != SsiType::Gssi
            && Some(c.carrier_num) == carrier
            && c.ts_assigned.iter().any(|set| *set)
            && c.ts_assigned == owners.map(|o| o == Some(addr.ssi))
    }

    /// Whether a packet-data channel assignment of `ssi` to `ts_assigned` of `carrier` still gives
    /// it its channel: that of this carrier, or of the packet-data carrier (`pdch_handoff_owners`),
    /// as the UMAC set them before this slot. A call may have taken a slot (`allocate_any_slot`),
    /// or the channel may have gone or changed, while the assignment waited in its queue. A quit
    /// (no timeslot) and an allocation to a carrier whose channels this scheduler does not know
    /// are left as they are.
    fn pdch_assignment_still_valid(&self, ssi: u32, carrier: u16, ts_assigned: &[bool; 4]) -> bool {
        if !ts_assigned.iter().any(|set| *set) {
            return true;
        }
        if carrier == self.carrier_num {
            *ts_assigned == self.pdch_timeslots_of(ssi)
        } else if Some(carrier) == self.pdch_handoff_carrier {
            *ts_assigned == self.pdch_handoff_owners.map(|o| o == Some(ssi))
        } else {
            true
        }
    }

    /// The packet-data channel assignments to `pdch_handoff_carrier` that went out since the
    /// last call: (ISSI, timeslots, downlink slot that carried it).
    pub fn take_assignments_elsewhere(&mut self) -> Vec<(u32, [bool; 4], TdmaTime)> {
        std::mem::take(&mut self.pdch_assignments_elsewhere)
    }

    /// Downlink PDUs queued on the packet-data channels of this scheduler.
    pub fn pdch_queue_depth(&self) -> usize {
        (1..=4u8)
            .filter(|ts| self.pdch_owner(*ts).is_some() && self.queue_index(*ts) == usize::from(*ts) - 1)
            .map(|ts| self.dltx_queues[usize::from(ts) - 1].len())
            .sum()
    }

    /// The radio `ssi` moves to its packet-data channel of another carrier: what is queued here
    /// for it alone (MCCH and deferred items). Grants and random-access acknowledgements are void
    /// there and dropped, a fragmentation under way is dropped (the MS discards a partial TM-SDU
    /// when it moves, EN 300 392-2 23.5.4.3.1; the LLC retransmits an acknowledged one), a
    /// packet-data channel assignment stays; everything else is returned, in order, for the
    /// channel's queue. Returns the items and how many were dropped.
    pub fn take_individual_items(&mut self, ssi: u32) -> (Vec<DlSchedElem>, usize) {
        let (handoff, owners) = (self.pdch_handoff_carrier, self.pdch_handoff_owners);
        let mine = |e: &DlSchedElem| match e {
            DlSchedElem::Resource(pdu, ..) => {
                !Self::is_handoff_assignment(pdu, handoff, owners) && pdu.addr.is_some_and(|a| a.ssi == ssi && a.ssi_type != SsiType::Gssi)
            }
            DlSchedElem::FragBuf(f) => f.ssi() == Some(ssi) && !f.is_for_group(),
            DlSchedElem::Grant(addr, ..) | DlSchedElem::RandomAccessAck(addr) => addr.ssi == ssi,
            DlSchedElem::Broadcast(_) | DlSchedElem::Stealing(..) => false,
        };
        let mut taken = Vec::new();
        let mut dropped = 0;
        let mut queue = std::mem::take(&mut self.dltx_next_slot_queue);
        queue.append(&mut self.dltx_queues[0]);
        let mut kept = Vec::with_capacity(queue.len());
        for e in queue {
            if !mine(&e) {
                kept.push(e);
                continue;
            }
            match e {
                DlSchedElem::Grant(..) | DlSchedElem::RandomAccessAck(_) => dropped += 1,
                // Dropping a started fragger reports its TM-SDU discarded.
                DlSchedElem::FragBuf(f) if f.is_started() => dropped += 1,
                e => taken.push(e),
            }
        }
        self.dltx_queues[0] = kept;
        (taken, dropped)
    }

    /// The items `take_individual_items` took from the MCCH, first in the queue of the
    /// packet-data channel of `ssi` here, in their order. Nothing when it has none here.
    pub fn enqueue_on_pdch(&mut self, ssi: u32, items: Vec<DlSchedElem>) {
        let Some(ts) = (1..=4u8).find(|ts| self.is_pdch_of(*ts, ssi)) else {
            return;
        };
        let q = self.queue_index(ts);
        let rest = std::mem::take(&mut self.dltx_queues[q]);
        self.dltx_queues[q] = items;
        self.dltx_queues[q].extend(rest);
    }

    /// ISSI whose packet-data channel `ts` is.
    pub fn pdch_owner(&self, ts: u8) -> Option<u32> {
        (1..=4).contains(&ts).then(|| self.pdch[ts as usize - 1]).flatten()
    }

    /// Whether `ts` is the packet-data channel of `ssi`.
    pub fn is_pdch_of(&self, ts: u8, ssi: u32) -> bool {
        self.pdch_owner(ts) == Some(ssi)
    }

    /// Timeslots of the packet-data channel of `ssi` on this carrier.
    pub fn pdch_timeslots_of(&self, ssi: u32) -> [bool; 4] {
        [1u8, 2, 3, 4].map(|ts| self.is_pdch_of(ts, ssi))
    }

    /// The owner of `ts` when its packet-data channel has more than one slot.
    fn multislot_owner(&self, ts: u8) -> Option<u32> {
        let owner = self.pdch_owner(ts)?;
        (self.pdch.iter().filter(|o| **o == Some(owner)).count() > 1).then_some(owner)
    }

    /// Lowest-numbered slot of the packet-data channel `ts` belongs to for `ssi` (the key of its
    /// uplink reassembly and of its downlink queue); None when `ts` is not a slot of it.
    pub fn pdch_anchor(&self, ts: u8, ssi: u32) -> Option<u8> {
        if !self.is_pdch_of(ts, ssi) {
            return None;
        }
        (1..=4u8).find(|t| self.is_pdch_of(*t, ssi))
    }

    /// Index of the downlink queue slot `ts` takes from and fills: its channel's lowest slot on a
    /// packet-data channel of several slots, whose slots all carry the same downlink (EN 300 392-2
    /// 23.3.5, 23.4.2.1.1 NOTE 3), else its own.
    fn queue_index(&self, ts: u8) -> usize {
        match self.multislot_owner(ts).and_then(|owner| self.pdch_anchor(ts, owner)) {
            Some(anchor) => anchor as usize - 1,
            None => ts as usize - 1,
        }
    }

    /// Before a slot of a packet-data channel of several slots is built: PDUs queued outside a
    /// build for the next slot (`dl_enqueue_tma_next_frame`, the MCCH channel-allocation deferral)
    /// are meant for ts1, where the radios they are for listen, not for this channel. (On a
    /// carrier without MCCH queue 0 may be a channel's queue: nothing is built outside a build
    /// there.) They go behind what waits there, as at the end of a build of ts1.
    fn pdch_route_out_of_build_items(&mut self, ts: TdmaTime) {
        if !self.allow_mcch() {
            return;
        }
        if self.dltx_next_slot_queue.is_empty() || self.multislot_owner(ts.t).is_none() {
            return;
        }
        self.dltx_queues[0].append(&mut self.dltx_next_slot_queue);
    }

    /// Drop the resources, fragments, grants and random-access acknowledgements queued on `ts`
    /// for `ssi`, and the group PDUs copied there for it (others stay). Returns how many were
    /// dropped.
    fn dl_drop_queued_for_ssi(&mut self, ts: u8, ssi: u32) -> usize {
        let queue = &mut self.dltx_queues[ts as usize - 1];
        let before = queue.len();
        queue.retain(|elem| {
            let for_ssi = match elem {
                DlSchedElem::Resource(pdu, ..) => pdu.addr.is_some_and(|a| a.ssi == ssi || a.ssi_type == SsiType::Gssi),
                DlSchedElem::FragBuf(fragger) => fragger.ssi() == Some(ssi) || fragger.is_for_group(),
                DlSchedElem::Grant(addr, _, _) | DlSchedElem::RandomAccessAck(addr) => addr.ssi == ssi,
                DlSchedElem::Broadcast(_) | DlSchedElem::Stealing(..) => false,
            };
            if for_ssi
                && let DlSchedElem::Resource(_, _, Some(reporter), _) = elem
                && reporter.get_state() == tetra_core::TxState::Pending
            {
                reporter.mark_discarded();
            }
            !for_ssi
        });
        before - queue.len()
    }

    pub fn can_deliver_stealing(&self, ts: u8) -> bool {
        self.supports_assigned_traffic_ts(ts) && self.circuits.is_active(Direction::Dl, self.carrier_num, ts)
    }

    fn generate_hangtime_idle_schf(&self) -> BitBuffer {
        // Full-slot SCH/F carrying a Null PDU (idle).
        let mut buf = BitBuffer::new(SCH_F_CAP);
        let pdu = MacResource::null_pdu();
        pdu.to_bitbuf(&mut buf);
        buf
    }

    // pub fn set_scrambling_code(&mut self, scrambling_code: u32) {
    //     self.scrambling_code = scrambling_code;
    //     unimplemented!("need to refresh some msgs possibly");
    // }

    // pub fn set_precomputed_msgs(&mut self, precomps: PrecomputedUmacPdus) {
    //     self.precomps = precomps;
    //     unimplemented!("need to refresh some msgs possibly");
    // }

    /// Update the System Wide Services flag in the broadcast SYSINFO.
    pub fn set_system_wide_services_state(&mut self, enabled: bool) {
        if self.precomps.mle_sysinfo.bs_service_details.system_wide_services != enabled {
            self.precomps.mle_sysinfo.bs_service_details.system_wide_services = enabled;
            // Should already be signalled at SwMI interface level
            tracing::debug!(
                "BsChannelScheduler: system_wide_services {}",
                if enabled { "ENABLED" } else { "DISABLED" }
            );
        }
    }

    /// Fully wipe the schedule
    pub fn purge_schedule(&mut self) {
        self.dltx_queues = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        self.ulsched = EMPTY_SCHED;
    }

    /// Sets the current downlink time to the given TdmaTime
    /// Wipes the schedule, as it can no longer be guaranteed to be valid
    pub fn set_dl_time(&mut self, new_ts: TdmaTime) {
        self.cur_dltime = new_ts;
        self.purge_schedule();
    }

    pub fn ul_ts_to_sched_index(&self, ts: &TdmaTime) -> usize {
        let to_index = (ts.f as usize - 1) + ((ts.m as usize - 1) * 18) + (ts.h as usize * 18 * 60);
        to_index % MACSCHED_NUM_FRAMES
    }

    ///////// UPLINK GRANT PROCESSING /////////

    /// Finds a grant opportunity for uplink transmission
    /// If num_slots is 1, is_halfslot may specifiy whether only a half slot is needed
    /// Returns (opportunities_to_skip, Vec<timestamps_of_granted_slots>)
    /// Returns None if no suitable opportunity is found in the schedule
    pub fn ul_find_grant_opportunity(&self, t: u8, num_slots: usize, is_halfslot: bool) -> Option<(usize, Vec<TdmaTime>)> {
        let first_opportunity = self.cur_dltime.forward_to_timeslot(t);
        let mut grant_timeslots = Vec::with_capacity(num_slots);
        let mut opportunities_skipped = 0;

        assert!(!is_halfslot || num_slots == 1, "is_halfslot set for num_slots > 1");

        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            // let candidate_t = self.cur_ts.add_timeslots(dist as i32 * 4);
            // Base off of internal perception of time, convert to UL time
            // Below may crash someday, but I'd want to investigate that situation
            let candidate_t = first_opportunity.add_timeslots(dist as i32 * 4);
            assert!(
                candidate_t.t == first_opportunity.t,
                "ul_find_grant_opportunity: candidate_t.ts {} does not match requested ts {}. Please report this to developer. ",
                candidate_t.t,
                first_opportunity.t
            );

            tracing::debug!(
                "ul_find_grant_opportunity: considering candidate ul_ts {}, have {:?}",
                candidate_t,
                grant_timeslots
            );

            if candidate_t.f == 18 {
                // Frame 18 is never granted — ACCESS-ASSIGN marks UL as CommonOnly on this frame,
                // and timing at the multiframe boundary caused grant delivery to fail. But the MS
                // counts it the way TS 100 392-2 23.5.2.2.2 says, and so must we:
                // - before the grant starts, every frame 18 slot (linearization ones included) is
                //   a delay opportunity it counts;
                // - inside a multi-slot grant it jumps only the linearization slot and would use
                //   any other frame 18 slot, so a run may not cross one: restart after it.
                // Skipping them uncounted made the MS transmit a frame before the slots we had
                // reserved, and its fragments were dropped as "unassigned block".
                if grant_timeslots.is_empty() {
                    opportunities_skipped += 1;
                } else if !candidate_t.is_mandatory_clch() {
                    opportunities_skipped += grant_timeslots.len() + 1;
                    grant_timeslots.clear();
                }
                continue;
            }

            if candidate_t.is_mandatory_clch() {
                // Not an opportunity; skip
                continue;
            }

            let index = self.ul_ts_to_sched_index(&candidate_t);
            let elem = &self.ulsched[t as usize - 1][index];
            // tracing::debug!("ul_find_grant_opportunity: sched[{}] ts {}: {:?}", index, candidate_t, elem);
            if (elem.ul1.is_none() && elem.ul2.is_none()) || (is_halfslot && (elem.ul1.is_none() || elem.ul2.is_none())) {
                // Free UL slot, add this timeslot to result vec
                grant_timeslots.push(candidate_t);
                // continue;
            } else {
                // Something is here, clear our grant timeslots
                opportunities_skipped += grant_timeslots.len() + 1;
                grant_timeslots.clear();
            }

            // Check if done
            if grant_timeslots.len() == num_slots {
                return Some((opportunities_skipped, grant_timeslots));
            }
        }

        // If we get here, we did not find a suitable grant opportunity
        None
    }

    /// Reserves all slots designated in a grant option
    /// If only one halfslot is needed, returns 1 or 2 designating which slot was reserved
    pub fn ul_reserve_grant(&mut self, ssi: u32, grant_timestamps: Vec<TdmaTime>, is_halfslot: bool, usage_marker: Option<u8>) -> u8 {
        assert!(!grant_timestamps.is_empty());
        assert!(!is_halfslot || grant_timestamps.len() == 1);
        // let ts = grant_timestamps[0].t as usize;
        for ts in grant_timestamps {
            let index = self.ul_ts_to_sched_index(&ts);

            let elem: &mut TimeslotSchedule = &mut self.ulsched[ts.t as usize - 1][index];
            // Stamp the usage marker on the slot. AACH generation for this
            // slot will then emit Traffic(marker) per ETSI §23.5.2, which tells
            // the MS that holds the reservation it can transmit here.
            if let Some(m) = usage_marker {
                elem.usage_marker = Some(m);
            }
            if is_halfslot {
                if elem.ul1.is_none() {
                    elem.ul1 = Some(ssi);
                    return 1;
                } else {
                    assert!(elem.ul2.is_none(), "ul_reserve_grant: ul2 already set for ts {:?}, ssi {}", ts, ssi);
                    elem.ul2 = Some(ssi);
                    return 2;
                }
            } else {
                assert!(elem.ul1.is_none(), "ul_reserve_grant: ul1 already set for ts {:?}, ssi {}", ts, ssi);
                assert!(elem.ul2.is_none(), "ul_reserve_grant: ul2 already set for ts {:?}, ssi {}", ts, ssi);
                elem.ul1 = Some(ssi);
                elem.ul2 = Some(ssi);
            }
        }

        // Full slots reserved
        0
    }

    /// Tries to find a way to satisfy a granting request, and reserves the slots in the schedule.
    /// On success returns a `BasicSlotgrant` plus an optional `usage_marker`. The marker is
    /// `Some(m)` only when the grant covers more than one slot — single-slot grants don't need
    /// one. The marker is stored on each reserved `TimeslotSchedule` entry so AACH generation
    /// for those slots emits `Traffic(m)` and the MS can identify its reservation.
    pub fn ul_process_cap_req(
        &mut self,
        timeslot: u8,
        addr: TetraAddress,
        res_req: &ReservationRequirement,
    ) -> Option<(BasicSlotgrant, Option<u8>)> {
        let is_halfslot = res_req == &ReservationRequirement::Req1Subslot;
        let requested_cap = if is_halfslot { 1 } else { res_req.to_req_slotcount() };
        if !is_halfslot && self.is_pdch_of(timeslot, addr.ssi) {
            // The reservation requirement is the radio's whole estimate (23.5.2.1): it replaces
            // whatever was still owed from an earlier one.
            self.pdch_ul_debt[timeslot as usize - 1] = None;
            return self.ul_grant_pdch_chunk(timeslot, addr, requested_cap);
        }

        // Find a suitable grant opportunity
        let grant_op = self.ul_find_grant_opportunity(timeslot, requested_cap, is_halfslot);

        tracing::debug!(
            "ul_process_cap_req: addr {}, res_req {:?}, requested_cap {}, is_halfslot {}, grant_op: {:?}",
            addr,
            res_req,
            requested_cap,
            is_halfslot,
            grant_op
        );

        // If found, reserve the slots and return a BasicSlotgrant + optional usage_marker.
        if let Some((skips, grant_timestamps)) = grant_op {
            // Granting delay is a 4-bit field with valid N of 1..13 (EN 300 392-2 cl. 21.5.6):
            // 14/15 are reserved codes and >=16 truncates. Telling the MS a wrong transmit time
            // is worse than not granting, so defer -- the MS repeats its capacity request.
            if skips > MAX_GRANTING_DELAY_OPPORTUNITIES {
                tracing::warn!(
                    "ul_process_cap_req: grant for {} would need a granting delay of {} opportunities (max {}), deferring",
                    addr,
                    skips,
                    MAX_GRANTING_DELAY_OPPORTUNITIES
                );
                return None;
            }

            // For multi-slot full grants, allocate a usage marker. We do this
            // BEFORE reserving so the marker can be embedded in the schedule.
            // Single-slot or half-slot grants don't need a marker — the MS
            // either has nothing to fragment (subslot) or completes the burst
            // in the one slot (single full slot).
            let usage_marker = if !is_halfslot && requested_cap >= 2 {
                Some(self.alloc_usage_marker(timeslot))
            } else {
                None
            };

            // Reserve the target granting opportunity. Get subslot (only relevant for halfslot reservation)
            let subslot = self.ul_reserve_grant(addr.ssi, grant_timestamps, is_halfslot, usage_marker);

            // tracing::info!("After grant:")
            // self.dump_ul_schedule_full(false);

            // Build BasicSlotgrant response element
            let cap_alloc = if res_req == &ReservationRequirement::Req1Subslot {
                match subslot {
                    1 => BasicSlotgrantCapAlloc::FirstSubslotGranted,
                    2 => BasicSlotgrantCapAlloc::SecondSubslotGranted,
                    _ => unreachable!("ul_process_cap_req: subslot must be 1 or 2, got {}", subslot),
                }
            } else {
                BasicSlotgrantCapAlloc::from_req_slotcount(requested_cap)
            };
            let grant_delay = if skips == 0 {
                BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity
            } else {
                BasicSlotgrantGrantingDelay::DelayNOpportunities(skips as u8)
            };
            Some((
                BasicSlotgrant {
                    capacity_allocation: cap_alloc,
                    granting_delay: grant_delay,
                },
                usage_marker,
            ))
        } else {
            tracing::warn!(
                "ul_process_cap_req: no suitable grant opportunity found for addr {}, res_req {:?}",
                addr,
                res_req
            );
            None
        }
    }

    /// Grants the radio on its packet-data channel `timeslot` up to `PDCH_UL_GRANT_CHUNK_SLOTS` of
    /// the `requested` slots, the largest count a basic slot grant can express that fits, and
    /// notes the rest as owed so `tick_start` grants it once this chunk has gone by.
    fn ul_grant_pdch_chunk(&mut self, timeslot: u8, addr: TetraAddress, requested: usize) -> Option<(BasicSlotgrant, Option<u8>)> {
        let wanted = requested.min(PDCH_UL_GRANT_CHUNK_SLOTS);
        let Some((granted, skips, grant_timestamps)) = (1..=wanted).rev().find_map(|n| {
            self.ul_find_grant_opportunity(timeslot, n, false)
                .map(|(skips, slots)| (n, skips, slots))
        }) else {
            tracing::warn!(
                "ul_process_cap_req: no PDCH ts {} slot free for {} ({} wanted)",
                timeslot,
                addr,
                requested
            );
            return None;
        };
        if skips > MAX_GRANTING_DELAY_OPPORTUNITIES {
            tracing::warn!(
                "ul_process_cap_req: PDCH grant for {} would need a granting delay of {} opportunities, deferring",
                addr,
                skips
            );
            return None;
        }
        let usage_marker = (granted >= 2).then(|| self.alloc_usage_marker(timeslot));
        let last_slot = *grant_timestamps.last().expect("a grant has slots");
        self.ul_reserve_grant(addr.ssi, grant_timestamps, false, usage_marker);
        self.pdch_ul_debt[timeslot as usize - 1] = (granted < requested).then_some((addr, requested - granted, last_slot));
        tracing::debug!(
            "ul_process_cap_req: PDCH ts {} grants {} {} of {} slot(s), delay {}",
            timeslot,
            addr,
            granted,
            requested,
            skips
        );
        Some((
            BasicSlotgrant {
                capacity_allocation: BasicSlotgrantCapAlloc::from_req_slotcount(granted),
                granting_delay: if skips == 0 {
                    BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity
                } else {
                    BasicSlotgrantGrantingDelay::DelayNOpportunities(skips as u8)
                },
            },
            usage_marker,
        ))
    }

    /// Grants the next chunk of the capacity still owed on each packet-data channel once the
    /// radio has had the slots of the previous one.
    fn ul_grant_pdch_debts(&mut self) {
        for idx in 0..4 {
            let Some((addr, remaining, last_slot)) = self.pdch_ul_debt[idx] else {
                continue;
            };
            let ts = idx as u8 + 1;
            if last_slot.age(self.cur_dltime) < 0 {
                continue;
            }
            self.pdch_ul_debt[idx] = None;
            if !self.is_pdch_of(ts, addr.ssi) {
                continue;
            }
            if let Some((grant, usage_marker)) = self.ul_grant_pdch_chunk(ts, addr, remaining) {
                self.dl_enqueue_grant(ts, addr, grant, usage_marker);
            }
        }
    }

    /// Returns schedule info for the given uplink timeslot and full-or-subslot
    /// If Both is requested, schedule is assumed to have matching allocation for two subslots
    /// If not, a warning is issued and None is returned.
    pub fn ul_get_slot_owner(&self, ts: TdmaTime, slot: PhyBlockNum) -> Option<u32> {
        let sched = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
        match slot {
            PhyBlockNum::Block1 => sched.ul1,
            PhyBlockNum::Block2 => sched.ul2,
            PhyBlockNum::Both => {
                if sched.ul1 != sched.ul2 {
                    tracing::warn!("ul_get_slot_owner: requested Both but ul1 {:?} != ul2 {:?}", sched.ul1, sched.ul2);
                    return None;
                }
                sched.ul1
            }
            _ => unreachable!(),
        }
    }

    fn ul_get_usage(&self, ts: TdmaTime) -> AccessAssignUlUsage {
        let ul_sched = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
        match (ul_sched.ul1, ul_sched.ul2) {
            // A reserved slot with a usage_marker gets `Traffic(marker)` so the
            // MS that holds the reservation can identify its slot from AACH and
            // continue a fragmented uplink burst (MacFragUl → MacEndUl). Without
            // a marker, the MS abandons the burst after one slot — see the
            // comment on `next_usage_marker` for the failure mode this fixes.
            (Some(_), Some(_)) => {
                if let Some(marker) = ul_sched.usage_marker {
                    AccessAssignUlUsage::Traffic(marker)
                } else {
                    AccessAssignUlUsage::AssignedOnly
                }
            }
            (Some(_), None) => {
                if let Some(marker) = ul_sched.usage_marker {
                    AccessAssignUlUsage::Traffic(marker)
                } else {
                    AccessAssignUlUsage::CommonAndAssigned
                }
            }
            (None, None) => AccessAssignUlUsage::CommonOnly,
            _ => unreachable!("ul2 can't be set with ul1 None"),
        }
    }

    /// Allocate a fresh usage marker for a multi-slot reservation in `timeslot`.
    /// The marker is taken from the per-timeslot rotating cursor in the valid
    /// range [4, 62]. ETSI reserves 0 (Unallocated), 1-3, and 63 (Common
    /// linearisation), so we skip those.
    ///
    /// We don't track outstanding markers — the cursor just wraps. With only
    /// a handful of in-flight reservations per timeslot at any moment and 59
    /// valid markers to choose from, accidental reuse is improbable, and even
    /// if it happens the consequence is benign (the other MS would see its
    /// marker re-issued in a different slot and re-attempt).
    fn alloc_usage_marker(&mut self, timeslot: u8) -> u8 {
        let idx = (timeslot as usize - 1).min(3);
        let marker = self.next_usage_marker[idx];
        // Advance cursor, wrapping in [4, 62].
        let next = if marker >= 62 { 4 } else { marker + 1 };
        self.next_usage_marker[idx] = next;
        marker
    }

    ////////// DOWNLINK SCHEDULING /////////

    /// Total queued downlink scheduling elements across all timeslots plus the next-slot carry-over.
    /// A cheap backlog gauge for the health monitor's Congestion domain (read once per tick).
    pub fn dl_queue_depth(&self) -> usize {
        self.dltx_queues.iter().map(|q| q.len()).sum::<usize>() + self.dltx_next_slot_queue.len()
    }

    /// Registers that we should transmit a MAC-RESOURCE or similar with a grant, somewhere this tick.
    /// `usage_marker` is set when the grant covers >1 slot — the MS uses it to identify the reservation
    /// when continuing the burst on the second slot (per ETSI §21.4.3.2). Single-slot grants pass None.
    pub fn dl_enqueue_grant(&mut self, ts: u8, addr: TetraAddress, grant: BasicSlotgrant, usage_marker: Option<u8>) {
        if ts == 1 && !self.allow_mcch() && !self.circuits.is_active(Direction::Dl, self.carrier_num, ts) && !self.is_pdch_of(1, addr.ssi) {
            tracing::debug!(
                "dl_enqueue_grant: carrier={} ignoring TS1 grant for {} because MCCH is disabled in mode {:?}",
                self.carrier_num,
                addr,
                self.downlink_mode
            );
            return;
        }
        tracing::debug!(
            "dl_enqueue_grant: ts {} enqueueing PDU {:?} for addr {} marker {:?}",
            ts,
            grant,
            addr,
            usage_marker
        );
        let elem = DlSchedElem::Grant(addr, grant, usage_marker);
        let q = self.queue_index(ts);
        self.dltx_queues[q].push(elem);
    }

    pub fn dl_enqueue_random_access_ack(&mut self, ts: u8, addr: TetraAddress) {
        if ts == 1 && !self.allow_mcch() && !self.circuits.is_active(Direction::Dl, self.carrier_num, ts) && !self.is_pdch_of(1, addr.ssi) {
            tracing::debug!(
                "dl_enqueue_random_access_ack: carrier={} ignoring TS1 random-access ack for {} because MCCH is disabled in mode {:?}",
                self.carrier_num,
                addr,
                self.downlink_mode
            );
            return;
        }
        tracing::debug!(
            "dl_enqueue_random_access_ack: ts {} enqueueing random access acknowledgementfor addr {}",
            ts,
            addr
        );
        let elem = DlSchedElem::RandomAccessAck(addr);
        let q = self.queue_index(ts);
        self.dltx_queues[q].push(elem);
    }

    fn identify_timeslots_for_ssi(&self, addr: Option<TetraAddress>, link_id: LinkId) -> [u8; NUM_TIMESLOTS] {
        let Some(addr) = addr else {
            tracing::warn!("identify_timeslots_for_ssi: MAC-RESOURCE has no address, dropping");
            return [0, 0, 0, 0];
        };

        // `bearer = "pdch"`: group signalling copied to a member's packet-data channel goes there.
        if addr.ssi_type == SsiType::Gssi
            && let Ok(ts) = u8::try_from(link_id)
            && self.pdch_owner(ts).is_some()
            && !self.circuits.is_active(Direction::Dl, self.carrier_num, ts)
        {
            return [ts, 0, 0, 0];
        }

        if addr.ssi_type == SsiType::Gssi || link_id == 0 {
            if self.allow_mcch() {
                return [1, 0, 0, 0];
            }
            tracing::debug!(
                "identify_timeslots_for_ssi: carrier={} mode {:?} has no MCCH for addr {}, dropping linkless/common-control routing",
                self.carrier_num,
                self.downlink_mode,
                addr
            );
            return [0, 0, 0, 0];
        }

        let Ok(link_ts) = u8::try_from(link_id) else {
            tracing::warn!(
                "identify_timeslots_for_ssi: invalid link_id {} for {}, {}",
                link_id,
                addr,
                if self.allow_mcch() {
                    "defaulting to ts1"
                } else {
                    "dropping on carrier without MCCH"
                }
            );
            return if self.allow_mcch() { [1, 0, 0, 0] } else { [0, 0, 0, 0] };
        };

        if !(1..=NUM_TIMESLOTS as u8).contains(&link_ts) {
            tracing::warn!(
                "identify_timeslots_for_ssi: link_id {} is outside TS range for {}, {}",
                link_id,
                addr,
                if self.allow_mcch() {
                    "defaulting to ts1"
                } else {
                    "dropping on carrier without MCCH"
                }
            );
            return if self.allow_mcch() { [1, 0, 0, 0] } else { [0, 0, 0, 0] };
        }

        if self.circuits.is_active(Direction::Dl, self.carrier_num, link_ts) {
            if self.allow_mcch() {
                tracing::debug!(
                    "identify_timeslots_for_ssi: link TS {} is active DL traffic for {}, routing normal signaling on ts1",
                    link_ts,
                    addr
                );
                return [1, 0, 0, 0];
            }
            tracing::debug!(
                "identify_timeslots_for_ssi: carrier={} has no MCCH, routing assigned-channel signaling for {} on active DL traffic ts {}",
                self.carrier_num,
                addr,
                link_ts
            );
            return [link_ts, 0, 0, 0];
        }

        // Downlink ts1 of a packet-data channel of this carrier is its owner's.
        if !self.allow_mcch() && link_ts == 1 && !self.is_pdch_of(1, addr.ssi) {
            tracing::debug!(
                "identify_timeslots_for_ssi: carrier={} has no MCCH and link_ts=1 for {}, dropping",
                self.carrier_num,
                addr
            );
            return [0, 0, 0, 0];
        }

        [link_ts, 0, 0, 0]
    }

    fn dl_enqueue_tma_on_timeslots(
        &mut self,
        timeslots: [u8; NUM_TIMESLOTS],
        pdu: MacResource,
        sdu: BitBuffer,
        tx_reporter: Option<TxReporter>,
        pdch_assignment: bool,
    ) {
        // Queue the message for all timeslots on which we should transmit this message.
        // The loop basically prevents cloning the last element.
        for i in 0..NUM_TIMESLOTS {
            let ts = timeslots[i];
            if ts == 0 {
                if i == 0 {
                    tracing::debug!(
                        "dl_enqueue_tma_on_timeslots: carrier={} dropping unschedulable {:?} (mode {:?})",
                        self.carrier_num,
                        pdu,
                        self.downlink_mode
                    );
                }
                break;
            }
            let next_ts = if i < NUM_TIMESLOTS - 1 { timeslots[i + 1] } else { 0 };
            assert!(ts > 0);

            // If this PDU carries a chan_alloc element (DConnect/DConnectAck MCCH), check if we
            // already sent one this frame. DConnect MCCH (113 bits) + DConnectAck MCCH (110 bits)
            // = 223 bits > 216-bit slot capacity. Defer the second one to the next frame.
            let deferred = if ts == 1 && self.allow_mcch() && pdu.chan_alloc_element.is_some() {
                if self.mcch_chan_alloc_sent_this_frame {
                    true // Defer this one to next frame
                } else {
                    self.mcch_chan_alloc_sent_this_frame = true;
                    false // First one goes normally
                }
            } else {
                false
            };

            tracing::debug!(
                "dl_enqueue_tma: ts {}{} enqueueing PDU {:?} SDU {}",
                ts,
                if tx_reporter.is_some() { " reported" } else { "" },
                pdu,
                sdu.dump_bin(),
            );

            if deferred {
                tracing::debug!("dl_enqueue_tma: ts {} deferring chan_alloc PDU to next frame (slot capacity)", ts);
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, pdch_assignment);
                self.dltx_next_slot_queue.push(elem);
                break;
            } else if next_ts > 0 {
                // There is another ts for which we need to transmit this message.
                // Clone the message now and push it to the current ts.
                let elem = DlSchedElem::Resource(pdu.clone(), sdu.clone(), tx_reporter.clone(), pdch_assignment);
                let q = self.queue_index(ts);
                self.dltx_queues[q].push(elem);
            } else {
                // This is the last ts on which we need to transmit this message
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, pdch_assignment);
                let q = self.queue_index(ts);
                self.dltx_queues[q].push(elem);
                break;
            }
        }
    }

    pub fn dl_enqueue_tma(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let timeslots = self.identify_timeslots_for_ssi(pdu.addr, 0);
        self.dl_enqueue_tma_on_timeslots(timeslots, pdu, sdu, tx_reporter, false);
    }

    pub fn dl_enqueue_tma_for_link(&mut self, link_id: LinkId, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let timeslots = self.identify_timeslots_for_ssi(pdu.addr, link_id);
        self.dl_enqueue_tma_on_timeslots(timeslots, pdu, sdu, tx_reporter, false);
    }

    /// A packet-data channel assignment from SNDCP (`[packet_data] bearer = "pdch"`): queued like
    /// any resource, but marked so that it is not sent with its channel allocation once that
    /// channel is no longer its radio's (see `pdch_assignment_still_valid`).
    pub fn dl_enqueue_pdch_assignment_for_link(
        &mut self,
        link_id: LinkId,
        pdu: MacResource,
        sdu: BitBuffer,
        tx_reporter: Option<TxReporter>,
    ) {
        let timeslots = self.identify_timeslots_for_ssi(pdu.addr, link_id);
        self.dl_enqueue_tma_on_timeslots(timeslots, pdu, sdu, tx_reporter, true);
    }

    /// A packet-data TM-SDU (`[packet_data]`): it waits in line like any resource, and once its
    /// fragmentation has started it lets signalling that fits whole go before its fragments (see
    /// `dl_take_next_sched_item`).
    pub fn dl_enqueue_packet_data_for_link(&mut self, link_id: LinkId, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let ts = self.identify_timeslots_for_ssi(pdu.addr, link_id)[0];
        let fragger = BsFragger::new(pdu, sdu, tx_reporter).for_packet_data();
        if ts == 0 {
            // Dropping the fragger reports the TM-SDU discarded.
            tracing::debug!(
                "dl_enqueue_packet_data: carrier={} dropping unschedulable packet data",
                self.carrier_num
            );
            return;
        }
        let q = self.queue_index(ts);
        self.dltx_queues[q].push(DlSchedElem::FragBuf(fragger));
    }

    /// Enqueue a pre-built STCH block for FACCH/stealing on a traffic timeslot.
    /// The block must be 124 type1 bits containing MAC-U-SIGNAL header + TM-SDU. `to_talker` marks
    /// a PDU for the MS transmitting on `ts`, sent only in the frames it monitors.
    pub fn dl_enqueue_stealing(&mut self, ts: u8, block: BitBuffer, tx_reporter: Option<TxReporter>, to_talker: bool) {
        tracing::info!("dl_enqueue_stealing: ts {} enqueueing STCH block ({} bits)", ts, block.get_len());
        self.dltx_queues[ts as usize - 1].push(DlSchedElem::Stealing(block, tx_reporter, None, to_talker));
    }

    /// Enqueue two STCH blocks that steal both halves of one traffic slot: a MAC-RESOURCE that
    /// starts fragmentation (LI=111111) and its MAC-END.
    pub fn dl_enqueue_stealing_pair(&mut self, ts: u8, first: BitBuffer, second: BitBuffer, to_talker: bool) {
        tracing::info!(
            "dl_enqueue_stealing: ts {} enqueueing both halves of one slot ({} + {} bits)",
            ts,
            first.get_len(),
            second.get_len()
        );
        self.dltx_queues[ts as usize - 1].push(DlSchedElem::Stealing(first, None, Some(second), to_talker));
    }

    fn dl_enqueue_tma_frag_next_frame(&mut self, fragger: BsFragger) {
        tracing::debug!("dl_enqueue_tma_frag_next_frame: enqueueing {:?}", fragger);
        let elem = DlSchedElem::FragBuf(fragger);
        self.dltx_next_slot_queue.push(elem);
    }

    /// Enqueue a TMA PDU to be transmitted on the NEXT frame (ts1, frame N+1).
    /// Use this to deliberately separate two MCCH messages that would overflow the slot
    /// if sent together (e.g. DConnect MCCH + DConnectAck MCCH = 223 bits > 216-bit slot).
    pub fn dl_enqueue_tma_next_frame(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        tracing::debug!(
            "dl_enqueue_tma_next_frame: deferring PDU {:?} SDU {} to next frame",
            pdu,
            sdu.dump_bin()
        );
        let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter, false);
        self.dltx_next_slot_queue.push(elem);
    }

    pub fn dl_schedule_tmb(&mut self, _traffic: BitBuffer, _ts: &TdmaTime) {
        unimplemented!("Broadcast scheduling not implemented yet");
    }

    // pub fn dl_schedule_tmd(&mut self, _traffic: BitBuffer, _ts: &TdmaTime) {
    //     unimplemented!("Traffic scheduling not implemented yet");
    // }

    pub fn dl_schedule_tmd(&mut self, ts: u8, block: Vec<u8>) {
        self.circuits.put_block(self.carrier_num, ts, block);
    }

    pub fn circuit_is_active(&self, dir: Direction, ts: u8) -> bool {
        self.circuits.is_active(dir, self.carrier_num, ts)
    }

    /// Downlink voice blocks waiting to be transmitted on this timeslot.
    pub fn dl_queued_block_count(&self, ts: u8) -> usize {
        self.circuits.queued_block_count(self.carrier_num, ts)
    }

    pub fn duplex_peer_route(&self, ts: u8) -> Option<(u16, u8)> {
        self.circuits.get_ul_peer_route(self.carrier_num, ts)
    }

    pub fn dl_media_source(&self, ts: u8) -> Option<CircuitDlMediaSource> {
        self.circuits.get_dl_media_source(self.carrier_num, ts)
    }

    pub fn close_circuit(&mut self, dir: Direction, ts: u8) -> Option<Circuit> {
        // Clearing hangtime here is safe: if the circuit is gone, this timeslot is no longer in use.
        if (1..=4).contains(&ts) {
            self.hangtime[ts as usize - 1] = false;
        }
        self.circuits.close_circuit(dir, self.carrier_num, ts)
    }

    pub fn create_circuit(&mut self, dir: Direction, circuit: Circuit) {
        if !self.supports_assigned_traffic_ts(circuit.ts) {
            tracing::warn!(
                "BsChannelScheduler::create_circuit: rejecting {:?} circuit on carrier={} ts {} in mode {:?}",
                dir,
                self.carrier_num,
                circuit.ts,
                self.downlink_mode
            );
            return;
        }
        // New/updated circuit implies traffic mode.
        if (1..=4).contains(&circuit.ts) {
            self.hangtime[circuit.ts as usize - 1] = false;
        }
        self.circuits.create_circuit(dir, circuit);
    }

    /// Takes a block or None value.
    /// If block is present and some signalling channel, and space is available,
    /// adds a trailing Null PDU.
    /// If blk is None, returns None.
    /// Otherwise, returns blk unchanged (eg. for SYNC, broadcast, etc).
    pub fn try_add_null_pdus(&mut self, blk: Option<TmvUnitdataReq>) -> Option<TmvUnitdataReq> {
        // A null pdu in a slot:
        // 0000000000010000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000
        // Oddly, the fill_bits ind is set to 0, while a fill bit is indeed present to fill the slot.
        // We replicate that behavior here.
        if let Some(mut b) = blk {
            // STCH: MAC-U-SIGNAL occupies entire half-slot (3-bit header + 121-bit TM-SDU).
            // No additional MAC PDUs may be concatenated; receiver passes all bits after header to LLC.
            // Adding a null PDU would corrupt TM-SDU (misinterpreted as optional CMCE element flags).
            if b.logical_channel == LogicalChannel::SchHd || b.logical_channel == LogicalChannel::SchF {
                if b.mac_block.get_len_remaining() >= NULL_PDU_LEN_BITS {
                    tracing::trace!("try_add_null_pdus: closing blk with Null PDU");

                    // We have room for a Null PDU
                    let mut null_pdu = MacResource::null_pdu();
                    null_pdu.length_ind = 2; // Null PDU is 16 bits
                    let _ = null_pdu.update_len_and_fill_ind(0);
                    null_pdu.to_bitbuf(&mut b.mac_block);

                    // TODO FIXME: it's possibly the best idea to still add fill bits trailing this null pdu.
                    // Check real-world captures.
                } else {
                    tracing::debug!(
                        "try_add_null_pdus: not enough space for Null PDU in block, got {} bits remaining",
                        b.mac_block.get_len_remaining()
                    );
                }
            }

            Some(b)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the first scheduled resource for the given timeslot and address
    pub fn dl_get_scheduled_resource_for_ssi(&mut self, ts: TdmaTime, addr: &TetraAddress) -> Option<&mut DlSchedElem> {
        let q = self.queue_index(ts.t);
        let queue = &mut self.dltx_queues[q];

        for index in 0..queue.len() {
            let elem = &mut queue[index];
            if let DlSchedElem::Resource(pdu, _sdu, _repeat, _) = elem {
                if let Some(pdu_ssi) = pdu.addr {
                    if pdu_ssi.ssi == addr.ssi {
                        // Found a resource for this address
                        return queue.get_mut(index);
                    }
                }
            }
        }
        // No resource for this address was found
        None
    }

    /// Make a minimal resource to contain a grant or a random access acknowledgement
    pub fn dl_make_minimal_resource(addr: &TetraAddress, grant: Option<BasicSlotgrant>, random_access_ack: bool) -> MacResource {
        let mut pdu = MacResource {
            fill_bits: false, // updated later
            pos_of_grant: 0,
            encryption_mode: 0,
            random_access_flag: random_access_ack,
            length_ind: 0, // updated later
            addr: Some(*addr),
            event_label: None,
            usage_marker: None,
            power_control_element: None,
            slot_granting_element: grant,
            chan_alloc_element: None,
        };
        pdu.update_len_and_fill_ind(0);
        pdu
    }

    /// Takes and removes all grants and random access acknowledgements from the given timeslot's queue, returning them as a vec.
    pub fn dl_take_all_grants_and_acks(&mut self, timeslot: u8) -> Vec<DlSchedElem> {
        let q = self.queue_index(timeslot);
        let queue = &mut self.dltx_queues[q];
        let mut taken = Vec::new();

        let mut i = 0;
        while i < queue.len() {
            if matches!(queue[i], DlSchedElem::Grant(..) | DlSchedElem::RandomAccessAck(_)) {
                let elem = queue.remove(i);
                taken.push(elem);
            } else {
                i += 1;
            }
        }
        taken
    }

    /// Removes all elements from the schedule, except stolen blocks. This function is used
    /// when leaving hangtime to clear out any stale grants, resources, etc that can only be processed in signaling mode,
    /// while keeping stealing blocks that may still need to be transmitted via FACCH.
    /// Discarded elements are reported as such via tx_reporter if available. Returns true if elements were discarded.
    pub fn dl_drop_all_except_stolen(&mut self, timeslot: u8) -> bool {
        let queue = &mut self.dltx_queues[timeslot as usize - 1];
        let mut i = 0;
        let mut item_was_discarded = false;
        while i < queue.len() {
            if matches!(queue[i], DlSchedElem::Stealing(..)) {
                i += 1;
            } else {
                // Found a to-be-discarded element.
                // Remove, log, and call tx_reporter::mark_discarded() if applicable.
                // Logged at debug because this fires during normal hangtime entry/exit
                // races and isn't an anomaly worth surfacing as a warning. Per
                // proxiboi69 in MidnightBlueLabs/tetra-bluestation PR #85.
                let elem = queue.remove(i);
                item_was_discarded = true;
                tracing::debug!("dl_drop_all_except_stolen: discarding scheduled {:?} on ts {}", elem, timeslot);

                match elem {
                    DlSchedElem::Resource(_, _, tx_reporter, _) => {
                        // Report as discarded manually
                        if let Some(tx_reporter) = tx_reporter {
                            tx_reporter.mark_discarded();
                        }
                    }

                    DlSchedElem::FragBuf(_) => {
                        // Fragger self-marks any unsent fragments as discarded when dropped, so we don't need to do anything here.
                    }

                    DlSchedElem::RandomAccessAck(_) => {
                        // Dropped: the acknowledgement of a random access in hangtime not sent
                        // before traffic resumed. Its radio got its answer (a PTT its D-TX
                        // GRANTED, unflagged as it always was) or tries again where it may; kept
                        // for the next STCH to it, it would flag one that answers nothing, in a
                        // later call even (which some radios reject).
                    }

                    DlSchedElem::Grant(..) | DlSchedElem::Broadcast(_) => {
                        // Silently dropped as internal or not equipped with a tx_reporter
                    }
                    _ => unreachable!(),
                }
            }
        }

        item_was_discarded
    }

    pub fn dl_integrate_sched_elems_for_timeslot(&mut self, ts: TdmaTime) {
        // Remove all grants and acks from queue and collect them into a vec
        let grants_and_acks = self.dl_take_all_grants_and_acks(ts.t);

        // Process grants and acks
        for elem in grants_and_acks {
            // Try to find existing resource for this address
            let addr = match &elem {
                DlSchedElem::Grant(addr, _, _) => addr,
                DlSchedElem::RandomAccessAck(addr) => addr,
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            };
            // No grant or acknowledgement rides on a packet-data channel assignment to the
            // packet-data carrier: it would be one on the channel the MS leaves, and delay the
            // move (EN 300 392-2 23.5.4.3.1 rule 1 b); it goes on its own. (Calls are not
            // concerned: their channel allocations merge as always.)
            let (handoff, owners) = (self.pdch_handoff_carrier, self.pdch_handoff_owners);
            let mac_resource = self
                .dl_get_scheduled_resource_for_ssi(ts, addr)
                .filter(|e| !matches!(e, DlSchedElem::Resource(pdu, ..) if Self::is_handoff_assignment(pdu, handoff, owners)));
            match mac_resource {
                Some(DlSchedElem::Resource(pdu, _sdu, _repeat, _)) => {
                    // Integrate grant into the resource
                    match &elem {
                        DlSchedElem::Grant(_, grant, usage_marker) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Integrating grant {:?} into resource for addr {} marker {:?}",
                                grant,
                                addr,
                                usage_marker,
                            );
                            pdu.slot_granting_element = Some(grant.clone());
                            // Carry the marker through so the MS knows what to
                            // tag its reservation with on the next UL slot.
                            // Don't overwrite a marker we already set (e.g.
                            // when the grant came after an ACK that already
                            // populated it).
                            if pdu.usage_marker.is_none() {
                                pdu.usage_marker = *usage_marker;
                            }
                        }
                        DlSchedElem::RandomAccessAck(_) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Integrating ack into resource for addr {}",
                                addr
                            );
                            pdu.random_access_flag = true;
                        }
                        _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
                    }
                }
                None => {
                    // No resource for this address was found, create a new one

                    let pdu = match &elem {
                        DlSchedElem::Grant(_, grant, usage_marker) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Creating new resource for addr {} with grant {:?} marker {:?}",
                                addr,
                                grant,
                                usage_marker,
                            );
                            let mut pdu = Self::dl_make_minimal_resource(addr, Some(grant.clone()), false);
                            pdu.usage_marker = *usage_marker;
                            pdu
                        }
                        DlSchedElem::RandomAccessAck(_) => {
                            tracing::debug!(
                                "dl_integrate_sched_elems_for_timeslot: Creating new resource for addr {} with ack",
                                addr
                            );
                            Self::dl_make_minimal_resource(addr, None, true)
                        }
                        _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
                    };

                    // Push new resource into the queue. These do not need a tx_reporter
                    let dlsched_res = DlSchedElem::Resource(pdu, BitBuffer::new(0), None, false);
                    let q = self.queue_index(ts.t);
                    self.dltx_queues[q].push(dlsched_res);
                }
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            }
        }
    }

    fn dl_build_block_from_signalling_schedule(&mut self, ts: TdmaTime) -> Option<BitBuffer> {
        let mut buf_opt = None;
        let q = self.queue_index(ts.t);
        // On a packet-data channel of several slots, no fragmentation starts in this block while
        // its radio is about to transmit (see `multislot`).
        let whole_only = self.pdch_whole_only(ts);

        while !self.dltx_queues[q].is_empty() {
            let room = buf_opt.as_ref().map_or(SCH_F_CAP, |buf: &BitBuffer| buf.get_len_remaining());
            let opt = self.dl_take_next_sched_item(ts, room, whole_only);

            match opt {
                Some(sched_elem) => {
                    match sched_elem {
                        DlSchedElem::Broadcast(_) => {
                            unimplemented_log!("finalize_ts_for_tick: Broadcast scheduling not implemented");
                        }

                        DlSchedElem::Resource(mut pdu, sdu, tx_reporter, pdch_assignment) => {
                            // A packet-data channel assignment whose channel a call took, or that
                            // went, meanwhile must not send its radio there: it goes without the
                            // allocation, so the radio's data stays on the MCCH (the SNDCP learns
                            // of the lost slot from the allocator and drops the channel).
                            if pdch_assignment
                                && let (Some(c), Some(addr)) = (pdu.chan_alloc_element.as_ref(), pdu.addr)
                                && !self.pdch_assignment_still_valid(addr.ssi, c.carrier_num, &c.ts_assigned)
                            {
                                tracing::info!(
                                    "dl_build_block_from_signalling_schedule: PDCH assignment for {} to carrier={} {:?} is no longer its channel, sent without it",
                                    addr,
                                    c.carrier_num,
                                    c.ts_assigned
                                );
                                pdu.chan_alloc_element = None;
                                pdu.update_len_and_fill_ind(sdu.get_len());
                            }
                            // Allocate bitbuf if not already done
                            let mut buf = buf_opt.unwrap_or_else(|| BitBuffer::new(SCH_F_CAP));
                            // Create fragger, either to send the whole PDU or to start fragmentation
                            let mut fragger = BsFragger::new(pdu, sdu, tx_reporter);
                            if !fragger.get_next_chunk(&mut buf) {
                                // Fragmentation was started and we have more chunks to send
                                // Enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame(fragger);
                            } else {
                                self.pdch_note_assignment(ts, &fragger);
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::FragBuf(mut fragger) => {
                            // Allocate bitbuf if not already done
                            let mut buf = buf_opt.unwrap_or_else(|| BitBuffer::new(SCH_F_CAP));
                            if !fragger.get_next_chunk(&mut buf) {
                                // Fragmentation was continued and we still have more chunks to send
                                // Re-enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame(fragger);
                            } else {
                                self.pdch_note_assignment(ts, &fragger);
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::Stealing(_, tx_reporter, _, _) => {
                            // Stealing items should only appear on traffic timeslots; discard if found here
                            tracing::warn!(
                                "dl_build_block_from_signalling_schedule: Stealing item found on non-traffic ts {}, discarding",
                                ts.t
                            );
                            if let Some(tx_reporter) = tx_reporter {
                                tx_reporter.mark_discarded();
                            }
                        }

                        _ => {
                            tracing::error!("UMAC: finalize_ts_for_tick: unexpected DlSchedElem type {:?}, skipping", sched_elem);
                        }
                    }
                }
                None => {
                    // No more items to process, we can finalize this timeslot
                    break;
                }
            }
        }

        // If any signalling could not be sent this slot, it should be in the next slot queue.
        // Drain next_slot_queue into the front of the current slot queue so deferred PDUs are
        // sent before any newly-arriving ones in the next frame.  Using extend instead of swap
        // avoids a panic when the current queue already contains items (e.g. two back-to-back
        // P2P calls each deferring a chan_alloc PDU within the same tick).
        // Fragments go on in this slot. PDUs held back outside a build for the next frame (the
        // MCCH chan_alloc deferral, `dl_enqueue_tma_next_frame`) are for the MCCH: on a carrier
        // with one only ts1 takes them, or they would go on (or get stuck behind a call on) a slot
        // no idle radio listens to. Those go behind what this build left in the queue (a channel
        // allocation waiting for room, see `dl_take_next_sched_item`), which is older.
        if !self.dltx_next_slot_queue.is_empty() {
            let mcch_wait = self.allow_mcch() && ts.t != 1;
            let (merged, held): (Vec<_>, Vec<_>) = std::mem::take(&mut self.dltx_next_slot_queue)
                .into_iter()
                .partition(|e| !(mcch_wait && matches!(e, DlSchedElem::Resource(..))));
            self.dltx_next_slot_queue = held;
            let (resources, mut merged): (Vec<_>, Vec<_>) = merged.into_iter().partition(|e| matches!(e, DlSchedElem::Resource(..)));
            let current = &mut self.dltx_queues[q];
            merged.extend(current.drain(..));
            merged.extend(resources);
            *current = merged;
        }

        buf_opt
    }

    /// Build traffic block for active circuit. Returns (tch_block, optional_stch_block,
    /// optional_second_stch_block):
    /// - tch_block: speech/silence (274 bits)
    /// - stch_block: STCH signaling (124 bits) for FACCH stealing (EN 300 392-2, clause 23.5)
    /// - second_stch_block: when both halves are stolen, the STCH for the second half
    /// Also reports transmission, if a TxReporter was attached to the DlSchedElem::Stealing element
    fn dl_build_traffic_block(&mut self, ts: TdmaTime) -> (BitBuffer, Option<BitBuffer>, Option<BitBuffer>) {
        // Get speech data or silence
        let tch_buf = if let Some(block) = self.circuits.take_block(self.carrier_num, ts.t) {
            // Raw ACELP speech (274 bits for TCH/S). The Vec may be LARGER (e.g. 280
            // bits) and is clamped down to TCH_S_CAP. But a SHORTER block (e.g. a
            // truncated/garbage frame off the network) must not be clamped UP — that
            // would push set_raw_end past capacity and panic. Fall back to silence.
            if block.len() * 8 >= TCH_S_CAP {
                let mut buf = BitBuffer::from_vec(block);
                buf.set_raw_end(buf.get_raw_start() + TCH_S_CAP);
                buf
            } else {
                tracing::warn!(
                    "DL traffic carrier={} ts={}: queued voice block only {} bytes (<{} bits), sending silence",
                    self.carrier_num,
                    ts.t,
                    block.len(),
                    TCH_S_CAP
                );
                BitBuffer::new(TCH_S_CAP)
            }
        } else {
            // No voice data queued — send silence frame (all zeros).
            // This is normal during hangtime or between voice bursts.
            BitBuffer::new(TCH_S_CAP)
        };

        // Check for FACCH/stealing: take a queued Stealing item (highest priority signaling). The
        // talker was given monitoring pattern 1, so it listens to the downlink only in frames
        // with MN mod 3 = FN mod 3 (TS 100 392-2 9.6, equation 9.22): a PDU for it waits for one.
        let talker_listens = ts.m % 3 == ts.f % 3;
        let (stch_opt, tx_reporter_opt, second_stch) = {
            let q = &mut self.dltx_queues[ts.t as usize - 1];
            if let Some(i) = q
                .iter()
                .position(|e| matches!(e, DlSchedElem::Stealing(_, _, _, to_talker) if !*to_talker || talker_listens))
            {
                match q.remove(i) {
                    DlSchedElem::Stealing(buf, tx_reporter, second, _) => (Some(buf), tx_reporter, second),
                    _ => unreachable!(),
                }
            } else {
                (None, None, None)
            }
        };

        // Warn about other queued signaling that can't be sent via stealing yet
        let q = &self.dltx_queues[ts.t as usize - 1];
        if stch_opt.is_none() && !q.is_empty() && !q.iter().any(|e| matches!(e, DlSchedElem::Stealing(..))) {
            tracing::warn!("dl_build_traffic_block: queued signaling on ts {} but no stealing item", ts.t);
        }

        // If desired, report transmission
        if let Some(tx_reporter) = tx_reporter_opt {
            tx_reporter.mark_transmitted();
        }

        (tch_buf, stch_opt, second_stch)
    }

    /// Return first queued grant.
    /// If none; return first in-progress fragmented message.
    /// If none; return first to-be-transmitted resource.
    /// If none, return None.
    pub fn dl_take_prioritized_sched_item(&mut self, ts: TdmaTime) -> Option<DlSchedElem> {
        self.dl_take_next_sched_item(ts, SCH_F_CAP, false)
    }

    /// Whether `elem` goes whole in `room` bits and leaves room for a fragment after it.
    fn fits_before_a_fragment(elem: &DlSchedElem, room: usize) -> bool {
        let len = match elem {
            DlSchedElem::Resource(pdu, sdu, ..) => pdu.compute_header_len() + sdu.get_len(),
            DlSchedElem::FragBuf(f) if !f.is_started() && !f.is_packet_data() => f.whole_len_bits(),
            _ => return false,
        };
        len.div_ceil(8) * 8 + MIN_SLOT_CAP_FOR_FRAG <= room
    }

    /// Whether `elem` goes whole in `room` bits, a packet-data TM-SDU not started yet included;
    /// an element of unknown length does not. As the fragger sends it: a PDU that fills the rest
    /// of the block to the bit goes whole too (its length is indicated as the next octet,
    /// 23.4.3.2), a full advanced link segment with a grant of several slots say.
    fn goes_whole(elem: &DlSchedElem, room: usize) -> bool {
        let len = match elem {
            DlSchedElem::Resource(pdu, sdu, ..) => Some(pdu.compute_header_len() + sdu.get_len()),
            DlSchedElem::FragBuf(f) if !f.is_started() => Some(f.whole_len_bits()),
            _ => None,
        };
        len.is_some_and(|len| len <= room)
    }

    /// Whether a resource that does not go whole in `room` bits can open its fragmentation there
    /// (see `BsFragger`); true for anything else.
    fn may_open_fragmentation(elem: &DlSchedElem, room: usize) -> bool {
        match elem {
            DlSchedElem::Resource(pdu, ..) => room >= MIN_SLOT_CAP_FOR_RES_FRAG_START && room >= pdu.compute_header_len(),
            _ => true,
        }
    }

    /// `dl_take_prioritized_sched_item` for a block with `room` bits still free. With
    /// `whole_only` nothing is taken that would start a fragmentation (a started one goes on).
    fn dl_take_next_sched_item(&mut self, ts: TdmaTime, room: usize, whole_only: bool) -> Option<DlSchedElem> {
        if ts.f == 18 {
            // No resources on frame 18
            return None;
        }

        // Map 1-based ts to 0-based index, bail on 0 or out of range.
        // (ts.t should always be 1..=4, but guard rather than unwrap so a bad ts can't
        // panic the scheduler.)
        if ts.t < 1 || (ts.t as usize) > self.dltx_queues.len() {
            tracing::warn!("dl_take_prioritized_sched_item: ts.t={} out of range, no item", ts.t);
            return None;
        }
        let slot = self.queue_index(ts.t);
        // On a packet-data channel of several slots frame 18 already leaves three channel slots
        // without a fragment, and N.203 is only ">= 4" (EN 300 392-2 annex B.2): next to it, in
        // frames 17 and 1, a started fragment opens the block and signalling waits behind it.
        let multislot = self.multislot_owner(ts.t).is_some();
        let let_in = !multislot || !matches!(ts.f, 1 | 17);
        let cold = &self.cold_carriers;
        let Some(q) = self.dltx_queues.get_mut(slot) else {
            return None;
        };

        // Return grants first
        if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::Grant(..))) {
            return Some(q.remove(i));
        }

        // Return FragBufs next. A packet-data TM-SDU not started yet waits in line with the
        // resources instead.
        if let Some(i) = q.iter().position(|e| {
            matches!(e, DlSchedElem::FragBuf(f) if f.is_started() || (!f.is_packet_data() && (!whole_only || Self::goes_whole(e, room))))
        }) {
            // A packet-data TM-SDU being fragmented lets signalling go first that fits whole in this
            // block and leaves room for its next fragment: the BS may interrupt a fragmented
            // message with non-fragmented ones (EN 300 392-2 23.4.2.1.1), and a fragment still goes
            // in every block. Not a channel allocation to a carrier warming up, nor what follows one
            // for its address (as below); a grant or random-access acknowledgement merged into one
            // goes on its own.
            if matches!(&q[i], DlSchedElem::FragBuf(f) if f.is_packet_data()) && let_in {
                let is_cold = |e: &DlSchedElem| Self::allocation_carrier(e).is_some_and(|c| cold.contains(&c));
                if let Some(k) = q.iter().position(|e| {
                    is_cold(e)
                        && matches!(e, DlSchedElem::Resource(pdu, ..)
                            if pdu.addr.is_some() && (pdu.slot_granting_element.is_some() || pdu.random_access_flag))
                }) && let DlSchedElem::Resource(pdu, ..) = &mut q[k]
                    && let Some(addr) = pdu.addr
                {
                    let own = Self::dl_make_minimal_resource(&addr, pdu.slot_granting_element.clone(), pdu.random_access_flag);
                    let own = DlSchedElem::Resource(own, BitBuffer::new(0), None, false);
                    if Self::fits_before_a_fragment(&own, room) {
                        pdu.slot_granting_element = None;
                        pdu.random_access_flag = false;
                        return Some(own);
                    }
                }
                let mut held = Vec::new();
                if let Some(j) = q.iter().position(|e| {
                    let e_ssi = match e {
                        DlSchedElem::Resource(pdu, ..) => pdu.addr.map(|a| a.ssi),
                        DlSchedElem::FragBuf(f) => f.ssi(),
                        _ => return false,
                    };
                    if is_cold(e) {
                        held.push(e_ssi);
                    }
                    Self::fits_before_a_fragment(e, room) && !held.contains(&e_ssi)
                }) {
                    return Some(q.remove(j));
                }
            }
            return Some(q.remove(i));
        }

        // Return Resources next. On a packet-data channel of several slots a resource that can
        // neither go whole nor open its fragmentation in the room left stays as it is for the next
        // slot of the channel (elsewhere it is queued again as a fragger, first in line).
        if let Some(i) = q.iter().position(|e| {
            (matches!(e, DlSchedElem::Resource(..)) || matches!(e, DlSchedElem::FragBuf(f) if f.is_packet_data() && !f.is_started()))
                && (!whole_only || Self::goes_whole(e, room))
                && (!multislot || Self::goes_whole(e, room) || Self::may_open_fragmentation(e, room))
        }) {
            // A channel allocation does not open a fragmentation it would not need in an empty
            // block: in a fragmented message it goes in the MAC-END, not in the MAC-RESOURCE (EN
            // 300 392-2 23.4.2.1.1), and a radio that obeys it at the end of the slot leaves before
            // the rest. It waits for the next slot (two D-SETUPs do not fit in one); what goes
            // whole behind it goes now, but not for its address or that of a PDU it would pass,
            // which keep their order. So does one to a carrier not on the air long enough yet
            // (`secondary_carrier_on_demand`): the radio would find nothing there to lock on to.
            let cold_carrier = Self::allocation_carrier(&q[i]).filter(|c| cold.contains(c));
            if let DlSchedElem::Resource(pdu, ..) = &q[i]
                && pdu.chan_alloc_element.is_some()
                && (cold_carrier.is_some() || (!Self::goes_whole(&q[i], room) && Self::goes_whole(&q[i], SCH_F_CAP)))
            {
                let ssi = pdu.addr.map(|a| a.ssi);
                if let Some(carrier) = cold_carrier {
                    tracing::debug!(
                        "dl_take_next_sched_item: channel allocation for {:?} waits for carrier {} to warm up",
                        ssi,
                        carrier
                    );
                } else {
                    tracing::debug!(
                        "dl_take_next_sched_item: channel allocation for {:?} waits for the next block ({} bits left)",
                        ssi,
                        room
                    );
                }
                // A grant merged into it is for this block (`ul_find_grant_opportunity`), or the
                // radio would transmit a frame after its slot: it goes now on its own, with the
                // address and random-access acknowledgement it had there, which leave it. So does a
                // random-access acknowledgement without a grant (WT, 23.5.1.4.8). Without
                // the allocation it has no usage marker (the allocation's is the call's; 23.5.5).
                if let DlSchedElem::Resource(pdu, ..) = &mut q[i]
                    && let Some(addr) = pdu.addr
                    && (pdu.slot_granting_element.is_some() || pdu.random_access_flag)
                {
                    let grant = pdu.slot_granting_element.clone();
                    let own = Self::dl_make_minimal_resource(&addr, grant.clone(), pdu.random_access_flag);
                    let own = DlSchedElem::Resource(own, BitBuffer::new(0), None, false);
                    if Self::goes_whole(&own, room) {
                        pdu.slot_granting_element = None;
                        pdu.random_access_flag = false;
                        return Some(own);
                    }
                }
                // Nothing overtakes a PDU for its address that stays: the addresses passed are held.
                // Nor does another channel allocation to a carrier warming up.
                let mut held = vec![ssi];
                let j = q.iter().skip(i + 1).position(|e| {
                    let e_ssi = match e {
                        DlSchedElem::Resource(pdu, ..) => pdu.addr.map(|a| a.ssi),
                        DlSchedElem::FragBuf(f) => f.ssi(),
                        _ => return false,
                    };
                    let pass = Self::goes_whole(e, room)
                        && !held.contains(&e_ssi)
                        && !Self::allocation_carrier(e).is_some_and(|c| cold.contains(&c));
                    if !pass {
                        held.push(e_ssi);
                    }
                    pass
                })?;
                return Some(q.remove(i + 1 + j));
            }
            return Some(q.remove(i));
        }

        // Return Stealing items last. They belong on traffic timeslots; surfacing them
        // here lets dl_build_block_from_signalling_schedule's Stealing arm discard any that
        // wrongly landed on a signalling slot, rather than leaving them queued forever
        // (which would also leak the traffic timeslot via has_pending_stealing).
        if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::Stealing(..))) {
            return Some(q.remove(i));
        }

        None
    }

    pub fn tick_start(&mut self, ts: TdmaTime) {
        // Increment current time
        self.cur_dltime = self.cur_dltime.add_timeslots(1);
        assert!(
            ts == self.cur_dltime,
            "BsChannelScheduler tick_start: ts mismatch, expected {}, got {}",
            self.cur_dltime,
            ts
        );
        self.ul_grant_pdch_debts();
    }

    /// Prepares a scheduled FUTURE timeslot for transfer to lmac and transmission
    /// Generates BBK block
    /// If the timeslot is not full, generates SYNC SB1/SB2 blocks.
    /// Increments cur_ts by one timeslot.
    /// Caller should check timestamp of returned DlTxElem to prevent desync
    pub fn finalize_ts_for_tick(&mut self) -> TmvUnitdataReqSlot {
        self.finalize_ts_for_tick_inner(true)
            .expect("primary carrier must always emit a downlink slot")
    }

    pub fn finalize_secondary_ts_for_tick(&mut self) -> Option<TmvUnitdataReqSlot> {
        self.finalize_ts_for_tick_inner(self.downlink_mode.emit_bcch())
    }

    fn clear_ul_schedule_for_tx_time(&mut self, ts: TdmaTime) {
        let index = self.ul_ts_to_sched_index(&ts.add_timeslots(-4));
        self.ulsched[ts.t as usize - 1][index].ul1 = None;
        self.ulsched[ts.t as usize - 1][index].ul2 = None;
        self.ulsched[ts.t as usize - 1][index].usage_marker = None;
    }

    fn finalize_ts_for_tick_inner(&mut self, emit_bcch: bool) -> Option<TmvUnitdataReqSlot> {
        // Reset the per-frame chan_alloc flag when we start processing ts1 (MCCH slot).
        // This allows the next DConnect MCCH to go normally while the subsequent DConnectAck
        // MCCH is deferred to the following frame.
        if self.cur_dltime.add_timeslots(MACSCHED_TX_AHEAD as i32).t == 1 && self.allow_mcch() {
            self.mcch_chan_alloc_sent_this_frame = false;
        }

        // We finalize a FUTURE slot: cur_ts plus some number of timeslots
        let ts = self.cur_dltime.add_timeslots(MACSCHED_TX_AHEAD as i32);
        let carrier_num = self.carrier_num;
        self.precomps.mac_sync.time = ts;
        self.precomps.mac_sysinfo1.hyperframe_number = Some(ts.h);
        self.precomps.mac_sysinfo2.hyperframe_number = Some(ts.h);

        let dl_circuit_active = self.circuits.is_active(Direction::Dl, self.carrier_num, ts.t) && ts.f != 18;
        let ul_circuit_active = self.circuits.is_active(Direction::Ul, self.carrier_num, ts.t) && ts.f != 18;

        // During hangtime we stop sending traffic frames and switch to signalling mode.
        // Keep traffic mode while FACCH/stealing is still queued for delivery.
        let hang_effective = if self.supports_assigned_traffic_ts(ts.t) {
            self.is_hangtime_effective(ts.t)
        } else {
            false
        };

        let dl_is_traffic = dl_circuit_active && !hang_effective;
        let ul_is_traffic = ul_circuit_active && !hang_effective;

        // Build the block for this timeslot with anything scheduled (traffic or signalling)
        // For traffic timeslots, also check for FACCH/stealing (STCH half-slot)
        let ul_phy = if ul_is_traffic { PhysicalChannel::Tp } else { PhysicalChannel::Cp };

        let mut elem = if dl_is_traffic {
            let (tch_buf, stch_opt, second_stch_opt) = self.dl_build_traffic_block(ts);

            if let Some(stch_buf) = stch_opt {
                tracing::info!(
                    "finalize_ts_for_tick: FACCH stealing on ts {} (stch={} bits, tch={} bits)",
                    ts.t,
                    stch_buf.get_len(),
                    tch_buf.get_len()
                );
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::Stch,
                        mac_block: stch_buf,
                        scrambling_code: self.scrambling_code,
                    }),
                    // Both halves stolen: the second carries the MAC-END and this slot's voice
                    // frame is not sent.
                    blk2: Some(match second_stch_opt {
                        Some(second) => TmvUnitdataReq {
                            logical_channel: LogicalChannel::Stch,
                            mac_block: second,
                            scrambling_code: self.scrambling_code,
                        },
                        None => TmvUnitdataReq {
                            logical_channel: LogicalChannel::TchS,
                            mac_block: tch_buf,
                            scrambling_code: self.scrambling_code,
                        },
                    }),
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else {
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::TchS,
                        mac_block: tch_buf,
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            }
        } else {
            self.pdch_route_out_of_build_items(ts);
            // The radio of a packet-data channel of several slots does not hear this slot while
            // it transmits or switches: nothing goes on it then (see `multislot`).
            let buf = if self.pdch_radio_hears(ts) {
                self.dl_integrate_sched_elems_for_timeslot(ts);
                // Its uplink capacity is granted in the slot that carries the grant.
                self.pdch_lazy_grant(ts);
                let buf = self.dl_build_block_from_signalling_schedule(ts);
                self.pdch_check_lazy_grant(ts);
                buf
            } else {
                None
            };
            if let Some(buf) = buf {
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: buf,
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else if hang_effective && dl_circuit_active {
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: self.generate_hangtime_idle_schf(),
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else if ts.f != 18
                && !dl_circuit_active
                && (self.pdch_owner(ts.t).is_some() || (!self.allow_mcch() && self.pdch_recently_released(ts)))
            {
                // An idle packet-data channel keeps sending SCH/F (a Null PDU) for its radio. On a
                // carrier without MCCH a slot that has just stopped being one does too, with the
                // AACH unallocated, which a radio still there reads (EN 300 392-2 9.5.1b NOTE).
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: self.generate_hangtime_idle_schf(),
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else if !emit_bcch {
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: self.generate_hangtime_idle_schf(),
                        scrambling_code: self.scrambling_code,
                    }),
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            } else {
                TmvUnitdataReqSlot {
                    carrier_num,
                    ts,
                    blk1: None,
                    blk2: None,
                    bbk: None,
                    ul_phy_chan: ul_phy,
                }
            }
        };

        // Sanity check: frame 18 should not carry user blocks
        if elem.blk1.is_some() && emit_bcch {
            assert!(ts.f != 18, "frame 18 shouldn't have blk1 set");
        }

        // Construct the BBK block to reflect UL/DL usage
        assert!(elem.bbk.is_none(), "BBK block already set");
        elem.bbk = Some(self.generate_bbk_block(ts));

        // tracing::trace!("finalize_ts_for_tick: have {}{}{}",
        //     if elem.bbk.is_some() { "bbk " } else { "" },
        //     if elem.blk1.is_some() { "blk1 " } else { "" },
        //     if elem.blk2.is_some() { "blk2 " } else { "" });

        // Populate blk1 if empty: BSCH on frame 18, SCH/HD on other frames
        if elem.blk1.is_none() {
            elem.blk1 = Some(self.generate_default_blks(ts));
        };

        // Check if second block may still be populated (blk1 is half-slot and blk2 is None)
        let blk1_lchan = elem.blk1.as_ref().unwrap().logical_channel;

        if blk1_lchan == LogicalChannel::Stch {
            // FACCH/Stealing: blk1 = STCH signaling, blk2 = TCH speech (already set above)
            assert!(elem.blk2.is_some(), "STCH blk1 must have blk2 (TCH half-slot)");
        } else if elem.blk2.is_none() && (blk1_lchan == LogicalChannel::Bsch || blk1_lchan == LogicalChannel::SchHd) {
            // Populate blk2 with SYSINFO if blk1 is half-slot (not STCH)
            // Check blk1 is indeed short (124 for half-slot or 60 for SYNC)
            assert!(elem.blk1.as_ref().unwrap().mac_block.get_len() <= 124);

            let mut buf = BitBuffer::new(124);

            // Write MAC-SYSINFO (alternating sysinfo1/sysinfo2), followed by MLE-SYSINFO
            if ts.t % 2 == 1 {
                self.precomps.mac_sysinfo1.to_bitbuf(&mut buf);
            } else {
                self.precomps.mac_sysinfo2.to_bitbuf(&mut buf);
            }
            self.precomps.mle_sysinfo.to_bitbuf(&mut buf);

            elem.blk2 = Some(TmvUnitdataReq {
                logical_channel: LogicalChannel::Bnch,
                mac_block: buf,
                scrambling_code: self.scrambling_code,
            })
        } else if elem.blk2.is_none() {
            // Full-slot block (TCH or SCH/F): just verify it fills both half slots
            assert!(
                elem.blk1.as_ref().unwrap().mac_block.get_len() >= 268,
                "blk1 should be full-slot but is too short"
            );
        }

        assert!(elem.bbk.is_some(), "BBK block is not set, this should not happen");
        assert!(elem.blk1.is_some(), "blk1 block is not set, this should not happen");

        // If signalling channels are here, and there is spare room, we need to close them with a Null pdu
        elem.blk1 = self.try_add_null_pdus(elem.blk1);
        elem.blk2 = self.try_add_null_pdus(elem.blk2);

        // Move all BitBuffer positions to the start of the window
        elem.bbk.as_mut().unwrap().mac_block.seek(0);
        elem.blk1.as_mut().unwrap().mac_block.seek(0);
        if let Some(blk2) = elem.blk2.as_mut() {
            blk2.mac_block.seek(0);
        }

        // tracing::warn!("start finalize");
        // self.dump_ul_schedule_full(true);

        // Clear UL schedule for this timeslot. Releasing the usage_marker
        // alongside ul1/ul2 keeps the marker pool from leaking — once both
        // slots of a reservation have been consumed, the marker is free to
        // be re-issued. (If a reservation extends over multiple frames this
        // gets called once per consumed slot pair, which is correct.)
        self.clear_ul_schedule_for_tx_time(ts);

        // tracing::warn!("end finalize");
        // self.dump_ul_schedule_full(true);

        // We now have our bbk, blk1 and (optional) blk2
        Some(elem)
    }

    fn generate_bbk_block(&self, ts: TdmaTime) -> TmvUnitdataReq {
        let (ul_traffic_usage, dl_traffic_usage) = if ts.f == 18 {
            (None, None)
        } else {
            (
                self.circuits.get_usage(Direction::Ul, self.carrier_num, ts.t),
                self.circuits.get_usage(Direction::Dl, self.carrier_num, ts.t),
            )
        };

        // Generate BBK block
        let mut aach_bb = BitBuffer::new(14);
        if ts.f != 18 {
            let mut aach = AccessAssign::default();

            match ts.t {
                1 if self.allow_common_control_aach() => {
                    // TS1 (MCCH) DL is always CommonControl — that doesn't
                    // change for individual reservations.
                    aach.dl_usage = AccessAssignDlUsage::CommonControl;

                    let ul_usage_for_slot = self.ul_get_usage(ts);
                    match ul_usage_for_slot {
                        AccessAssignUlUsage::Traffic(_) => {
                            aach.ul_usage = ul_usage_for_slot;
                        }
                        _ => {
                            aach.ul_usage = AccessAssignUlUsage::CommonOnly;
                            aach.f1_af1 = Some(AccessField {
                                access_code: 0,
                                base_frame_len: 4,
                            });
                            aach.f2_af2 = Some(AccessField {
                                access_code: 0,
                                base_frame_len: 4,
                            });
                        }
                    }
                }
                1..=4 => {
                    // Additional channels (TS2..TS4, and TS1 of a carrier without MCCH).
                    // Normal operation: Traffic(usage) when a circuit is active, else Unallocated.
                    // Hangtime: immediately switch AACH to AssignedControl so radios
                    // detect the end of traffic in the same frame as D-TX CEASED.
                    // The timeslot may still be in traffic mode (for STCH delivery) but
                    // the AACH reflects the new channel state.
                    let in_hangtime = self.supports_assigned_traffic_ts(ts.t) && self.hangtime[ts.t as usize - 1];

                    if in_hangtime && (dl_traffic_usage.is_some() || ul_traffic_usage.is_some()) {
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        // AssignedOnly (Header 2) allows random access for MSs on
                        // the assigned channel while blocking common control MSs.
                        aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                        aach.f2_af = Some(AccessField {
                            access_code: 0,
                            base_frame_len: 4,
                        });
                    } else if dl_traffic_usage.is_none() && ul_traffic_usage.is_none() && self.pdch_owner(ts.t).is_some() {
                        // Assigned packet-data channel: DL assigned control, UL for the assigned MS
                        // (Header 2). Never Header 3 with an uplink usage marker >= 4: the MS
                        // would take the channel as gone (EN 300 392-2 23.5.6.1.1 c).
                        // The uplink of ts1 of a carrier without MCCH is never used by a channel (its
                        // adjacent-channel copy would land on the MCCH uplink): shown reserved, so no
                        // random access there (23.5.1.4.2 b).
                        let ul = &self.ulsched[ts.t as usize - 1][self.ul_ts_to_sched_index(&ts)];
                        let reserved = ul.ul1.is_some() || ul.ul2.is_some() || ts.t == 1;
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                        aach.f2_af = Some(AccessField {
                            access_code: 0,
                            base_frame_len: if reserved { 0 } else { 4 },
                        });
                    } else {
                        aach.dl_usage = if let Some(usage) = dl_traffic_usage {
                            AccessAssignDlUsage::Traffic(usage)
                        } else {
                            AccessAssignDlUsage::Unallocated
                        };
                        aach.ul_usage = if let Some(usage) = ul_traffic_usage {
                            AccessAssignUlUsage::Traffic(usage)
                        } else {
                            AccessAssignUlUsage::Unallocated
                        };
                    }
                }
                _ => {
                    tracing::error!("UMAC: generate_bbk_block: invalid timeslot {} (expected 1-4)", ts.t);
                    return TmvUnitdataReq {
                        logical_channel: LogicalChannel::Aach,
                        mac_block: BitBuffer::new(14),
                        scrambling_code: self.scrambling_code,
                    };
                }
            }

            aach.to_bitbuf(&mut aach_bb);
        } else {
            // Fr18
            assert!(ul_traffic_usage.is_none() && dl_traffic_usage.is_none());
            let aach = if self.allow_common_control_aach() {
                AccessAssignFr18 {
                    ul_usage: AccessAssignUlUsage::CommonOnly,
                    f1_af1: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 1,
                    }),
                    f2_af2: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 0,
                    }),
                    ..Default::default()
                }
            } else {
                AccessAssignFr18 {
                    ul_usage: AccessAssignUlUsage::AssignedOnly,
                    f1_af1: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 0,
                    }),
                    f2_af2: Some(AccessField {
                        access_code: 0,
                        base_frame_len: 0,
                    }),
                    ..Default::default()
                }
            };
            // TODO FIXME: Access field defaults are possibly not great
            aach.to_bitbuf(&mut aach_bb);
        }

        TmvUnitdataReq {
            logical_channel: LogicalChannel::Aach,
            mac_block: aach_bb,
            scrambling_code: self.scrambling_code,
        }
    }

    fn generate_default_blks(&self, ts: TdmaTime) -> TmvUnitdataReq {
        // Frame 18 of a packet-data channel of a carrier without MCCH is its control frame: BSCH
        // only where (MN + TN) mod 4 = 3, BNCH (after an SCH/HD) where it is 1, normal SCH/F
        // bursts elsewhere (EN 300 392-2 9.5.2), so its radio never meets a synchronization burst
        // where it expects an SCH block (which would cost it a downlink TM-SDU, 23.4.3.1.1 ii).
        if ts.f == 18 && !self.allow_mcch() && self.pdch_owner(ts.t).is_some() {
            match (ts.m + ts.t) % 4 {
                3 => {}
                1 => {
                    let mut buf = BitBuffer::new(SCH_HD_CAP);
                    MacResource::null_pdu().to_bitbuf(&mut buf);
                    return TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchHd,
                        mac_block: buf,
                        scrambling_code: self.scrambling_code,
                    };
                }
                _ => {
                    return TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: self.generate_hangtime_idle_schf(),
                        scrambling_code: self.scrambling_code,
                    };
                }
            }
        }
        match (ts.f, ts.t) {
            (1..=17, 1) => {
                // Primary TS1 alternates between SCH/HD+BNCH and SCH/F null.
                // Secondary BCCH/no-MCCH carriers never advertise MCCH, so TS1 stays
                // on the BCCH-style SCH/HD+BNCH form instead of emitting SCH/F nulls.
                if !self.allow_mcch() || ts.f % 2 == 0 {
                    let mut buf1 = BitBuffer::new(SCH_HD_CAP);
                    let blk1 = MacResource::null_pdu();
                    blk1.to_bitbuf(&mut buf1);
                    TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchHd,
                        mac_block: buf1,
                        scrambling_code: self.scrambling_code,
                    }
                } else {
                    let mut buf = BitBuffer::new(SCH_F_CAP);
                    let blk = MacResource::null_pdu();
                    blk.to_bitbuf(&mut buf);
                    TmvUnitdataReq {
                        logical_channel: LogicalChannel::SchF,
                        mac_block: buf,
                        scrambling_code: self.scrambling_code,
                    }
                }
            }
            (1..=17, 2..=4) | (18, _) => {
                // SYNC + SYSINFO (added later)
                let mut buf = BitBuffer::new(60);
                self.precomps.mac_sync.to_bitbuf(&mut buf);
                self.precomps.mle_sync.to_bitbuf(&mut buf);
                TmvUnitdataReq {
                    logical_channel: LogicalChannel::Bsch,
                    mac_block: buf,
                    scrambling_code: scrambler::SCRAMB_INIT,
                }
            }
            _ => unreachable!("BUG: unhandled match variant -- should never be reached"), // never happens
        }
    }

    pub fn dump_ul_schedule(&self, skip_empty: bool) {
        let ts = self.cur_dltime;
        tracing::info!("Dumping uplink schedule for {}:", ts);
        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            let ts = ts.add_timeslots(dist as i32 * 4);
            let index = self.ul_ts_to_sched_index(&ts);
            let elem = &self.ulsched[ts.t as usize - 1][index];
            if skip_empty && elem.ul1.is_none() && elem.ul2.is_none() {
                continue;
            }
            tracing::info!("  Schedule {}: {:?}", ts, elem);
        }
    }

    pub fn dump_ul_schedule_full(&self, skip_empty: bool) {
        tracing::info!("Dumping uplink schedule for {}:", self.cur_dltime);

        for dist in 0..MACSCHED_NUM_FRAMES - 1 {
            let ts = self.cur_dltime.add_timeslots(dist as i32 * 4);
            let index = self.ul_ts_to_sched_index(&ts);
            if skip_empty
                && self.ulsched[0][index].ul1.is_none()
                && self.ulsched[0][index].ul2.is_none()
                && self.ulsched[1][index].ul1.is_none()
                && self.ulsched[1][index].ul2.is_none()
                && self.ulsched[2][index].ul1.is_none()
                && self.ulsched[2][index].ul2.is_none()
                && self.ulsched[3][index].ul1.is_none()
                && self.ulsched[3][index].ul2.is_none()
            {
                continue;
            }
            tracing::info!(
                "  Schedule {}: ({} / {})  ({} / {})  ({} / {})  ({} / {})",
                ts,
                self.ulsched[0][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[0][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[1][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[1][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[2][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[2][index].ul2.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[3][index].ul1.map_or("-".to_string(), |v| v.to_string()),
                self.ulsched[3][index].ul2.map_or("-".to_string(), |v| v.to_string())
            );
        }
    }

    pub fn dump_dl_queue(&self) {
        tracing::info!("Dumping downlink queue:");
        for (index, elem) in self.dltx_queues.iter().enumerate() {
            for e in elem {
                tracing::trace!("  ts[{}] {:?}", index, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use tetra_core::{
        address::{SsiType, TetraAddress},
        debug::setup_logging_default,
    };

    use tetra_pdus::{
        mle::{
            fields::bs_service_details::BsServiceDetails,
            pdus::{d_mle_sync::DMleSync, d_mle_sysinfo::DMleSysinfo},
        },
        umac::{
            enums::sysinfo_opt_field_flag::SysinfoOptFieldFlag,
            fields::{
                sysinfo_default_def_for_access_code_a::SysinfoDefaultDefForAccessCodeA, sysinfo_ext_services::SysinfoExtendedServices,
            },
            pdus::{access_assign::AccessAssign, mac_sync::MacSync, mac_sysinfo::MacSysinfo},
        },
    };

    use super::*;

    pub fn get_testing_slotter() -> BsChannelScheduler {
        let _guard = setup_logging_default(None);
        let ext_services = SysinfoExtendedServices {
            auth_required: false,
            class1_supported: true,
            class2_supported: true,
            class3_supported: false,
            sck_n: Some(0),
            dck_retrieval_during_cell_select: None,
            dck_retrieval_during_cell_reselect: None,
            linked_gck_crypto_periods: None,
            short_gck_vn: None,
            sdstl_addressing_method: 2,
            gck_supported: false,
            section: 0,
            section_data: 0,
        };

        let def_access = SysinfoDefaultDefForAccessCodeA {
            imm: 8,
            wt: 5,
            nu: 5,
            fl_factor: false,
            ts_ptr: 0,
            min_pdu_prio: 0,
        };

        let sysinfo1 = MacSysinfo {
            main_carrier: 1001,
            freq_band: 4,
            freq_offset_index: 0,
            duplex_spacing: 0,
            reverse_operation: false,
            num_of_csch: 0,
            ms_txpwr_max_cell: 5,
            rxlev_access_min: 3,
            access_parameter: 7,
            radio_dl_timeout: 3,
            cck_id: None,
            hyperframe_number: Some(0),
            option_field: SysinfoOptFieldFlag::DefaultDefForAccCodeA,
            ts_common_frames: None,
            default_access_code: Some(def_access),
            ext_services: None,
        };

        let sysinfo2 = MacSysinfo {
            main_carrier: sysinfo1.main_carrier,
            freq_band: sysinfo1.freq_band,
            freq_offset_index: sysinfo1.freq_offset_index,
            duplex_spacing: sysinfo1.duplex_spacing,
            reverse_operation: sysinfo1.reverse_operation,
            num_of_csch: sysinfo1.num_of_csch,
            ms_txpwr_max_cell: sysinfo1.ms_txpwr_max_cell,
            rxlev_access_min: sysinfo1.rxlev_access_min,
            access_parameter: sysinfo1.access_parameter,
            radio_dl_timeout: sysinfo1.radio_dl_timeout,
            cck_id: sysinfo1.cck_id,
            hyperframe_number: sysinfo1.hyperframe_number,
            option_field: SysinfoOptFieldFlag::ExtServicesBroadcast,
            ts_common_frames: None,
            default_access_code: None,
            ext_services: Some(ext_services),
        };

        let mle_sysinfo_pdu = DMleSysinfo {
            location_area: 2,
            subscriber_class: 65535, // All subscriber classes allowed
            bs_service_details: BsServiceDetails {
                registration: true,
                deregistration: true,
                priority_cell: false,
                no_minimum_mode: true,
                migration: false,
                system_wide_services: true,
                voice_service: true,
                circuit_mode_data_service: false,
                sndcp_service: false,
                aie_service: false,
                advanced_link: false,
            },
        };

        let mac_sync_pdu = MacSync {
            system_code: 1,
            colour_code: 1,
            time: TdmaTime::default(),
            sharing_mode: 0, // Continuous transmission
            ts_reserved_frames: 0,
            u_plane_dtx: false,
            frame_18_ext: false,
        };

        let mle_sync_pdu = DMleSync {
            mcc: 204,
            mnc: 1337,
            neighbor_cell_broadcast: 2,
            cell_load_ca: 0,
            late_entry_supported: true,
        };

        let precomps = PrecomputedUmacPdus {
            mac_sysinfo1: sysinfo1,
            mac_sysinfo2: sysinfo2,
            mle_sysinfo: mle_sysinfo_pdu,
            mac_sync: mac_sync_pdu,
            mle_sync: mle_sync_pdu,
        };

        let mut sched = BsChannelScheduler::new(1, precomps);
        sched.set_dl_time(TdmaTime::default().add_timeslots(2));
        sched
    }

    fn get_testing_secondary_scheduler() -> BsChannelScheduler {
        let mut sched = get_testing_slotter();
        sched.set_carrier_num(1002);
        sched.set_downlink_mode(CarrierDownlinkMode::SecondaryBcchNoMcch);
        sched
    }

    fn test_circuit(direction: Direction, ts: u8) -> Circuit {
        Circuit {
            direction,
            carrier_num: 1001,
            peer_carrier_num: None,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
        }
    }

    fn test_circuit_on_carrier(direction: Direction, carrier_num: u16, ts: u8) -> Circuit {
        Circuit {
            direction,
            carrier_num,
            peer_carrier_num: None,
            ts,
            peer_ts: None,
            usage: 4,
            circuit_mode: tetra_saps::control::enums::circuit_mode_type::CircuitModeType::TchS,
            speech_service: Some(0),
            etee_encrypted: false,
            dl_media_source: CircuitDlMediaSource::LocalLoopback,
        }
    }

    #[test]
    fn test_secondary_idle_ts1_emits_broadcast_not_idle_schf_only() {
        let mut sched = get_testing_secondary_scheduler();
        sched.set_dl_time(TdmaTime { t: 4, f: 2, m: 1, h: 0 });

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");

        assert_eq!(slot.carrier_num, 1002);
        assert_eq!(slot.ts.t, 1);
        assert_eq!(slot.blk1.as_ref().expect("blk1").logical_channel, LogicalChannel::SchHd);
        assert_eq!(slot.blk2.as_ref().expect("blk2").logical_channel, LogicalChannel::Bnch);
    }

    #[test]
    fn test_secondary_idle_ts2_emits_bsch_and_sysinfo() {
        let mut sched = get_testing_secondary_scheduler();
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");

        assert_eq!(slot.carrier_num, 1002);
        assert_eq!(slot.ts.t, 2);
        assert_eq!(slot.blk1.as_ref().expect("blk1").logical_channel, LogicalChannel::Bsch);
        assert_eq!(slot.blk2.as_ref().expect("blk2").logical_channel, LogicalChannel::Bnch);
    }

    #[test]
    fn test_secondary_sysinfo_keeps_primary_main_carrier() {
        let mut sched = get_testing_secondary_scheduler();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");
        let mut bnch = slot.blk2.expect("secondary ts1 should have BNCH").mac_block;
        let sysinfo = MacSysinfo::from_bitbuf(&mut bnch).expect("BNCH should start with MAC-SYSINFO");

        assert_eq!(sysinfo.main_carrier, 1001);
    }

    #[test]
    fn test_secondary_ts1_aach_is_not_common_control_common_only() {
        let mut sched = get_testing_secondary_scheduler();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");
        let mut bbk = slot.bbk.expect("bbk").mac_block;
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse");

        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Unallocated);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::Unallocated);
    }

    #[test]
    fn test_primary_ts1_aach_remains_common_control_common_only() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let slot = sched.finalize_ts_for_tick();
        let mut bbk = slot.bbk.expect("bbk").mac_block;
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse");

        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
    }

    #[test]
    fn test_secondary_never_queues_mcch_messages_on_ts1() {
        let mut sched = get_testing_secondary_scheduler();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200699,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        sched.dl_enqueue_tma_for_link(0, pdu, BitBuffer::new(0), None);
        sched.dl_enqueue_grant(1, addr, grant, None);
        sched.dl_enqueue_random_access_ack(1, addr);

        assert_eq!(sched.dltx_queues[0].len(), 0);
    }

    #[test]
    fn test_secondary_traffic_allocation_routes_to_traffic_slot_without_mcch() {
        let mut sched = get_testing_secondary_scheduler();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200699,
        };
        sched.create_circuit(Direction::Dl, test_circuit_on_carrier(Direction::Dl, 1002, 3));
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);

        sched.dl_enqueue_tma_for_link(3, pdu, BitBuffer::new(0), None);

        assert_eq!(sched.dltx_queues[0].len(), 0);
        assert_eq!(sched.dltx_queues[2].len(), 1);
    }

    #[test]
    fn test_secondary_facch_still_works_on_traffic_slot() {
        let mut sched = get_testing_secondary_scheduler();
        sched.create_circuit(Direction::Dl, test_circuit_on_carrier(Direction::Dl, 1002, 2));
        sched.set_dl_time(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None, false);

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");

        assert_eq!(slot.carrier_num, 1002);
        assert_eq!(slot.ts.t, 2);
        assert_eq!(slot.blk1.as_ref().expect("blk1").logical_channel, LogicalChannel::Stch);
        assert_eq!(slot.blk2.as_ref().expect("blk2").logical_channel, LogicalChannel::TchS);
    }

    #[test]
    fn test_secondary_assigned_ts1_emits_traffic_and_non_common_aach() {
        let mut sched = get_testing_secondary_scheduler();
        sched.create_circuit(Direction::Dl, test_circuit_on_carrier(Direction::Dl, 1002, 1));
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");
        let mut bbk = slot.bbk.expect("bbk").mac_block;
        let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH should parse");

        assert_eq!(slot.blk1.as_ref().expect("blk1").logical_channel, LogicalChannel::TchS);
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_ne!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        assert_ne!(aach.ul_usage, AccessAssignUlUsage::CommonOnly);
    }

    #[test]
    fn test_secondary_assigned_ts1_can_deliver_facch_stealing() {
        let mut sched = get_testing_secondary_scheduler();
        sched.create_circuit(Direction::Dl, test_circuit_on_carrier(Direction::Dl, 1002, 1));
        sched.set_dl_time(TdmaTime { t: 4, f: 1, m: 1, h: 0 });
        sched.dl_enqueue_stealing(1, BitBuffer::new(124), None, false);

        let slot = sched.finalize_secondary_ts_for_tick().expect("secondary should emit");

        assert_eq!(slot.blk1.as_ref().expect("blk1").logical_channel, LogicalChannel::Stch);
        assert_eq!(slot.blk2.as_ref().expect("blk2").logical_channel, LogicalChannel::TchS);
    }

    #[test]
    fn test_halfslot_grants() {
        let mut sched = get_testing_slotter();
        let resreq = ReservationRequirement::Req1Subslot;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let grant1 = sched.ul_process_cap_req(1, addr, &resreq);
        tracing::info!("grant1: {:?}", grant1);
        assert!(grant1.is_some(), "ul_process_cap_req should return Some, but got None");

        sched.dump_ul_schedule(false);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        let cap_alloc1 = grant1.unwrap().0.capacity_allocation;
        assert_eq!(
            cap_alloc1,
            BasicSlotgrantCapAlloc::FirstSubslotGranted,
            "ul_process_cap_req should return FirstSubslotGranted, but got {:?}",
            cap_alloc1
        );
        let grant2 = sched.ul_process_cap_req(1, addr, &resreq);
        tracing::info!("grant2: {:?}", grant2);
        assert!(grant2.is_some(), "ul_process_cap_req should return Some, but got None");
        let cap_alloc2 = grant2.unwrap().0.capacity_allocation;
        assert_eq!(
            cap_alloc2,
            BasicSlotgrantCapAlloc::SecondSubslotGranted,
            "ul_process_cap_req should return SecondSubslotGranted, but got {:?}",
            cap_alloc2
        );

        sched.dump_ul_schedule(false);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        sched.dump_ul_schedule(false);
    }

    #[test]
    fn test_halfslot_and_fullslot_grant() {
        let mut sched = get_testing_slotter();
        let resreq1 = ReservationRequirement::Req1Subslot;
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };

        sched.dump_ul_schedule(true);
        let grant1 = sched.ul_process_cap_req(1, addr, &resreq1);
        tracing::info!("grant1: {:?}", grant1);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        assert!(grant1.is_some());
        let cap_alloc1 = grant1.unwrap().0.capacity_allocation;
        assert_eq!(cap_alloc1, BasicSlotgrantCapAlloc::FirstSubslotGranted);

        sched.dump_ul_schedule(true);
        let resreq2 = ReservationRequirement::Req3Slots;
        let Some((grant2, _marker)) = sched.ul_process_cap_req(1, addr, &resreq2) else {
            tracing::error!("BUG: unexpected message or state -- routing error");
            return;
        };
        tracing::info!("grant2: {:?}", grant2);
        sched.dump_ul_schedule(true);

        let u1 = sched.ul_get_usage(TdmaTime { t: 1, f: 1, m: 1, h: 0 });
        let u2 = sched.ul_get_usage(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let u3 = sched.ul_get_usage(TdmaTime { t: 1, f: 3, m: 1, h: 0 });
        tracing::info!("usage ts 1/2/3: {:?}/{:?}/{:?}", u1, u2, u3);

        assert_eq!(grant2.capacity_allocation, BasicSlotgrantCapAlloc::Grant3Slots);
        assert_eq!(grant2.granting_delay, BasicSlotgrantGrantingDelay::DelayNOpportunities(1));
    }

    #[test]
    fn test_dl_tma_gssi_routes_to_mcch() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Gssi,
            ssi: 2200699,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);

        sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);

        assert_eq!(sched.dltx_queues[0].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 0);
    }

    #[test]
    fn test_dl_tma_linkless_issi_routes_to_mcch() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200699,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);

        sched.dl_enqueue_tma_for_link(0, pdu, sdu, None);

        assert_eq!(sched.dltx_queues[0].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 0);
    }

    #[test]
    fn test_dl_tma_issi_routes_by_link_timeslot() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200699,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);

        sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);

        assert_eq!(sched.dltx_queues[0].len(), 0);
        assert_eq!(sched.dltx_queues[2].len(), 1);
    }

    #[test]
    fn test_dl_tma_issi_avoids_active_dl_traffic_slot() {
        let mut sched = get_testing_slotter();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200699,
        };
        sched.create_circuit(Direction::Dl, test_circuit(Direction::Dl, 3));
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);

        sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);

        assert_eq!(sched.dltx_queues[0].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 0);
    }

    #[test]
    fn test_stealing_requires_active_dl_traffic_slot() {
        let mut sched = get_testing_slotter();

        assert!(!sched.can_deliver_stealing(2));

        sched.create_circuit(Direction::Ul, test_circuit(Direction::Ul, 2));
        assert!(!sched.can_deliver_stealing(2));

        sched.create_circuit(Direction::Dl, test_circuit(Direction::Dl, 2));
        assert!(sched.can_deliver_stealing(2));
    }

    #[test]
    fn test_non_traffic_stealing_is_discarded() {
        let mut sched = get_testing_slotter();

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None, false);
        assert!(sched.has_pending_stealing(2));

        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 2, f: 1, m: 1, h: 0 })
                .is_none()
        );
        assert!(!sched.has_pending_stealing(2));
    }

    /// A stolen PDU for the MS transmitting on the slot goes out only in a frame of monitoring
    /// pattern 1, MN mod 3 = FN mod 3 (TS 100 392-2 9.6); one for any other MS goes out at once.
    #[test]
    fn test_stealing_for_the_talker_waits_for_its_monitoring_frame() {
        let mut sched = get_testing_slotter();

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None, true);
        // Multiframe 1, frame 2: not a pattern-1 frame (MN mod 3 = 1, FN mod 3 = 2).
        let (_, stch, _) = sched.dl_build_traffic_block(TdmaTime { t: 2, f: 2, m: 1, h: 0 });
        assert!(stch.is_none(), "the talker does not listen in this frame");
        assert!(sched.has_pending_stealing(2));
        // Multiframe 1, frame 4: a pattern-1 frame.
        let (_, stch, _) = sched.dl_build_traffic_block(TdmaTime { t: 2, f: 4, m: 1, h: 0 });
        assert!(stch.is_some(), "the talker listens in this frame");

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None, false);
        let (_, stch, _) = sched.dl_build_traffic_block(TdmaTime { t: 2, f: 5, m: 1, h: 0 });
        assert!(stch.is_some(), "a PDU for another MS is not held back");
    }

    #[test]
    fn test_frame_18_keeps_stealing_queued() {
        let mut sched = get_testing_slotter();

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None, false);
        assert!(sched.has_pending_stealing(2));

        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 2, f: 18, m: 1, h: 0 })
                .is_none()
        );
        assert!(sched.has_pending_stealing(2));
    }

    #[test]
    fn test_frame_18_defers_signaling_on_same_timeslot_queue() {
        let mut sched = get_testing_slotter();
        let addr_ts2 = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200002,
        };
        let addr_ts3 = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 2200003,
        };
        let pdu_ts2 = BsChannelScheduler::dl_make_minimal_resource(&addr_ts2, None, false);
        let pdu_ts3 = BsChannelScheduler::dl_make_minimal_resource(&addr_ts3, None, false);

        sched.dl_enqueue_tma_for_link(2, pdu_ts2, BitBuffer::new(0), None);
        sched.dl_enqueue_tma_for_link(3, pdu_ts3, BitBuffer::new(0), None);

        assert_eq!(sched.dltx_queues[0].len(), 0);
        assert_eq!(sched.dltx_queues[1].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 1);

        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 2, f: 18, m: 1, h: 0 })
                .is_none()
        );
        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 3, f: 18, m: 1, h: 0 })
                .is_none()
        );

        assert_eq!(sched.dltx_queues[0].len(), 0);
        assert_eq!(sched.dltx_queues[1].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 1);

        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 1, f: 1, m: 2, h: 0 })
                .is_none()
        );

        assert_eq!(sched.dltx_queues[0].len(), 0);
        assert_eq!(sched.dltx_queues[1].len(), 1);
        assert_eq!(sched.dltx_queues[2].len(), 1);
    }

    /// The next slot `ts` (frames 1..=17) the scheduler finalizes, and its AACH.
    fn finalize_pdch_test_slot(sched: &mut BsChannelScheduler, ts: u8) -> (TmvUnitdataReqSlot, AccessAssign, u8) {
        loop {
            let next = sched.cur_dltime.add_timeslots(1);
            sched.tick_start(next);
            let slot = sched.finalize_ts_for_tick();
            if slot.ts.t != ts || slot.ts.f == 18 {
                continue;
            }
            let mut bbk = slot.bbk.as_ref().expect("bbk").mac_block.clone();
            let header = bbk.peek_bits(2).expect("AACH header") as u8;
            let aach = AccessAssign::from_bitbuf(&mut bbk).expect("AACH parses");
            return (slot, aach, header);
        }
    }

    #[test]
    fn test_pdch_aach_is_assigned_control_and_idle_blocks_are_schf() {
        let mut sched = get_testing_slotter();
        sched.set_pdch(4, Some(2260618));
        assert!(sched.is_pdch_of(4, 2260618) && !sched.is_pdch_of(3, 2260618));
        let (slot, aach, header) = finalize_pdch_test_slot(&mut sched, 4);
        assert_eq!(header, 2, "Header 2: downlink usage and an access field");
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
        assert_eq!(aach.ul_usage, AccessAssignUlUsage::AssignedOnly);
        assert_eq!(aach.f2_af.map(|af| af.base_frame_len), Some(4));
        assert_eq!(
            slot.blk1.as_ref().unwrap().logical_channel,
            LogicalChannel::SchF,
            "idle PDCH: SCH/F Null"
        );
        assert!(slot.blk2.is_none());

        // Not a PDCH (any more): unallocated, SYNC and SYSINFO as before.
        sched.set_pdch(4, None);
        let (slot, aach, _) = finalize_pdch_test_slot(&mut sched, 4);
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Unallocated);
        assert_eq!(slot.blk1.as_ref().unwrap().logical_channel, LogicalChannel::Bsch);
        // ts1 and the other slots never change.
        let (_, aach, _) = finalize_pdch_test_slot(&mut sched, 1);
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::CommonControl);
        sched.set_pdch(1, Some(2260618));
        assert_eq!(sched.pdch_owner(1), None, "ts1 is never a PDCH");
    }

    /// A multi-slot uplink reservation on the PDCH never turns its AACH into Header 3 with a
    /// traffic usage marker (the MS would leave the channel, EN 300 392-2 23.5.6.1.1 c): the
    /// reserved slots show "reserved" in the access field instead.
    #[test]
    fn test_pdch_uplink_reservation_never_advertises_a_usage_marker() {
        let mut sched = get_testing_slotter();
        sched.set_pdch(3, Some(2260618));
        let addr = TetraAddress::issi(2260618);
        let (_, marker) = sched
            .ul_process_cap_req(3, addr, &ReservationRequirement::Req4Slots)
            .expect("grant");
        assert!(marker.is_some(), "the grant keeps its usage marker, as on the MCCH");
        let mut reserved = 0;
        for _ in 0..8 {
            let (_, aach, header) = finalize_pdch_test_slot(&mut sched, 3);
            assert_ne!(header, 3, "never Header 3 on the PDCH");
            assert_eq!(aach.dl_usage, AccessAssignDlUsage::AssignedControl);
            if aach.f2_af.map(|af| af.base_frame_len) == Some(0) {
                reserved += 1;
            }
        }
        // The first reserved slot may already have been built (the MAC works a slot ahead).
        assert!(
            (3..=4).contains(&reserved),
            "the reserved slots are closed to random access: {reserved}"
        );
    }

    /// An MXP600 asks for 51 slots at once on its PDCH (a WSP Connect over the advanced link). No
    /// single grant in the schedule window holds that: grant it 4 slots at a time, the next chunk
    /// once the radio has had the previous one (EN 300 392-2 23.4.2.1.2 NOTE 9). Off a PDCH the
    /// request is refused as before.
    #[test]
    fn test_pdch_large_reservation_is_granted_in_chunks() {
        let addr = TetraAddress::issi(2145007);
        let mut plain = get_testing_slotter();
        assert!(
            plain.ul_process_cap_req(4, addr, &ReservationRequirement::Req51Slots).is_none(),
            "off a PDCH nothing changes"
        );

        let mut sched = get_testing_slotter();
        sched.set_pdch(4, Some(addr.ssi));
        let (grant, marker) = sched
            .ul_process_cap_req(4, addr, &ReservationRequirement::Req51Slots)
            .expect("a first chunk");
        assert_eq!(grant.capacity_allocation, BasicSlotgrantCapAlloc::Grant4Slots);
        assert!(marker.is_some(), "a multi-slot grant carries its usage marker");
        assert_eq!(sched.pdch_ul_debt[3].map(|(_, left, _)| left), Some(47));

        // Once the chunk's slots have gone by, the next chunk is granted.
        for _ in 0..(6 * 4) {
            let next = sched.cur_dltime.add_timeslots(1);
            sched.tick_start(next);
        }
        assert_eq!(sched.pdch_ul_debt[3].map(|(_, left, _)| left), Some(43));

        // A fresh reservation requirement replaces what was still owed.
        sched
            .ul_process_cap_req(4, addr, &ReservationRequirement::Req2Slots)
            .expect("grant");
        assert_eq!(sched.pdch_ul_debt[3], None);

        // Leaving the PDCH forgets the debt.
        sched
            .ul_process_cap_req(4, addr, &ReservationRequirement::Req51Slots)
            .expect("grant");
        sched.set_pdch(4, None);
        assert_eq!(sched.pdch_ul_debt[3], None);
    }

    #[test]
    fn test_circuit_and_hangtime_win_over_the_pdch() {
        let mut sched = get_testing_slotter();
        sched.set_pdch(2, Some(2260618));
        sched.create_circuit(Direction::Dl, test_circuit(Direction::Dl, 2));
        sched.create_circuit(Direction::Ul, test_circuit(Direction::Ul, 2));
        let (slot, aach, _) = finalize_pdch_test_slot(&mut sched, 2);
        assert_eq!(aach.dl_usage, AccessAssignDlUsage::Traffic(4));
        assert_eq!(slot.blk1.as_ref().unwrap().logical_channel, LogicalChannel::TchS);
        sched.set_hangtime(2, true);
        let (_, aach, header) = finalize_pdch_test_slot(&mut sched, 2);
        assert_eq!(
            (header, aach.dl_usage),
            (2, AccessAssignDlUsage::AssignedControl),
            "hangtime as without a PDCH"
        );
    }

    /// Group signalling the LLC copies to a member's PDCH goes on that slot, and only while it is
    /// a PDCH; losing the PDCH drops the copies still queued there.
    #[test]
    fn test_group_pdus_copied_to_a_pdch_go_there() {
        let mut sched = get_testing_slotter();
        let group = TetraAddress {
            ssi_type: SsiType::Gssi,
            ssi: 91,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&group, None, false);
        sched.dl_enqueue_tma_for_link(4, pdu, BitBuffer::new(0), None);
        assert_eq!((sched.dltx_queues[0].len(), sched.dltx_queues[3].len()), (1, 0), "no PDCH: MCCH");
        sched.set_pdch(4, Some(2260618));
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&group, None, false);
        sched.dl_enqueue_tma_for_link(4, pdu, BitBuffer::new(0), None);
        assert_eq!((sched.dltx_queues[0].len(), sched.dltx_queues[3].len()), (1, 1), "on the PDCH");
        sched.set_pdch(4, None);
        assert!(sched.dltx_queues[3].is_empty(), "dropped with the PDCH");
    }

    /// Losing the PDCH drops what was queued on it for its radio, and only that.
    #[test]
    fn test_losing_the_pdch_drops_only_its_radios_signalling() {
        let mut sched = get_testing_slotter();
        let (mine, other) = (TetraAddress::issi(2260618), TetraAddress::issi(2260619));
        sched.set_pdch(4, Some(mine.ssi));
        let reporter = TxReporter::new();
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&mine, None, false);
        sched.dl_enqueue_tma_for_link(4, pdu, BitBuffer::from_bitstr("0101"), Some(reporter.clone()));
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&other, None, false);
        sched.dl_enqueue_tma_for_link(4, pdu, BitBuffer::from_bitstr("0110"), None);
        sched.dl_enqueue_random_access_ack(4, mine);
        assert_eq!(sched.dltx_queues[3].len(), 3);
        sched.set_pdch(4, None);
        assert_eq!(reporter.get_state(), tetra_core::TxState::Discarded);
        assert_eq!(sched.dltx_queues[3].len(), 1);
        assert!(matches!(&sched.dltx_queues[3][0], DlSchedElem::Resource(pdu, ..) if pdu.addr == Some(other)));
    }

    #[test]
    fn test_dl_grant_and_ack_integration() {
        let mut sched = get_testing_slotter();
        let ts = TdmaTime::default();
        let addr = TetraAddress {
            ssi_type: SsiType::Issi,
            ssi: 1234,
        };
        let pdu = BsChannelScheduler::dl_make_minimal_resource(&addr, None, false);
        let sdu = BitBuffer::new(0);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let grant = BasicSlotgrant {
            capacity_allocation: BasicSlotgrantCapAlloc::FirstSubslotGranted,
            granting_delay: BasicSlotgrantGrantingDelay::CapAllocAtNextOpportunity,
        };

        sched.dl_enqueue_grant(ts.t, addr, grant, None);
        sched.dl_enqueue_random_access_ack(ts.t, addr);

        sched.dump_ul_schedule(true);
        sched.dump_dl_queue();

        assert!(sched.dltx_queues[ts.t as usize - 1].len() == 3);

        tracing::info!("Integrating queue");
        sched.dl_integrate_sched_elems_for_timeslot(ts);

        sched.dump_ul_schedule(true);
        sched.dump_dl_queue();

        assert!(sched.dltx_queues[ts.t as usize - 1].len() == 1);
    }

    /// A scheduler whose packet-data channel of `owner` is `slots` of the main carrier.
    pub(super) fn multislot_slotter(owner: u32, slots: &[u8]) -> BsChannelScheduler {
        let mut sched = get_testing_slotter();
        for ts in slots {
            sched.set_pdch(*ts, Some(owner));
        }
        sched
    }

    /// A scheduler of a carrier without MCCH (1002) whose packet-data channel of `owner` is
    /// `slots`.
    pub(super) fn carrier_slotter(owner: u32, slots: &[u8]) -> BsChannelScheduler {
        let mut sched = get_testing_slotter();
        sched.set_carrier_num(1002);
        sched.set_downlink_mode(CarrierDownlinkMode::SecondaryBcchNoMcch);
        for ts in slots {
            sched.set_pdch(*ts, Some(owner));
        }
        sched
    }

    /// The next `n` slots the scheduler builds.
    pub(super) fn finalize_slots(sched: &mut BsChannelScheduler, n: usize) -> Vec<TmvUnitdataReqSlot> {
        (0..n)
            .map(|_| {
                let next = sched.cur_dltime.add_timeslots(1);
                sched.tick_start(next);
                sched.finalize_ts_for_tick()
            })
            .collect()
    }

    /// The MAC PDUs of a downlink SCH/F block, in order: (MAC PDU kind, SSI, length indication)
    /// with kind 0 a MAC-RESOURCE, 2 a MAC-FRAG and 3 a MAC-END (no SSI).
    pub(super) fn dl_pdus(slot: &TmvUnitdataReqSlot) -> Vec<(u8, Option<u32>, u8)> {
        use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
        let Some(blk) = slot.blk1.as_ref().filter(|b| b.logical_channel == LogicalChannel::SchF) else {
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
            match b.peek_bits(3) {
                Some(0..=1) => {
                    let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
                    let Some(addr) = res.addr else { break };
                    out.push((0, Some(addr.ssi), res.length_ind));
                    if res.length_ind == 0b111111 {
                        break;
                    }
                    pos += res.length_ind as usize * 8;
                }
                Some(2) => {
                    out.push((2, None, 0));
                    break;
                }
                Some(3) => {
                    let Ok(end) = MacEndDl::from_bitbuf(&mut b) else { break };
                    out.push((3, None, end.length_ind));
                    pos += end.length_ind as usize * 8;
                }
                _ => break,
            }
        }
        out
    }

    /// The first octet of the TM-SDU of the first MAC-RESOURCE of a slot, and its SSI.
    fn first_resource_tag(slot: &TmvUnitdataReqSlot) -> Option<(u32, u64)> {
        let blk = slot.blk1.as_ref().filter(|b| b.logical_channel == LogicalChannel::SchF)?;
        let mut b = blk.mac_block.clone();
        b.seek(0);
        let res = MacResource::from_bitbuf(&mut b).ok()?;
        Some((res.addr?.ssi, b.read_bits(8)?))
    }

    /// A MAC-RESOURCE for `addr` whose TM-SDU starts with the octet `tag`, `bits` long.
    fn tagged_resource(addr: &TetraAddress, tag: u8, bits: usize) -> (MacResource, BitBuffer) {
        let mut sdu = format!("{tag:08b}");
        while sdu.len() < bits {
            sdu.push(if sdu.len() % 2 == 0 { '1' } else { '0' });
        }
        (
            BsChannelScheduler::dl_make_minimal_resource(addr, None, false),
            BitBuffer::from_bitstr(&sdu),
        )
    }

    /// Every slot of a packet-data channel of several slots takes its downlink from one queue,
    /// the one of its lowest slot, in order (EN 300 392-2 23.3.5).
    #[test]
    fn test_multislot_dl_takes_the_channel_queue_in_order() {
        let x = TetraAddress::issi(2_145_007);
        let mut sched = multislot_slotter(x.ssi, &[2, 3, 4]);
        for tag in 0..6u8 {
            let (pdu, sdu) = tagged_resource(&x, tag, 200);
            sched.dl_enqueue_tma_for_link(4, pdu, sdu, None);
        }
        assert_eq!(sched.dltx_queues[1].len(), 6, "all in the queue of ts2");
        let sent: Vec<(u8, u64)> = finalize_slots(&mut sched, 12)
            .iter()
            .filter_map(|s| first_resource_tag(s).filter(|r| r.0 == x.ssi).map(|r| (s.ts.t, r.1)))
            .collect();
        assert_eq!(sent, vec![(2, 0), (3, 1), (4, 2), (2, 3), (3, 4), (4, 5)]);
    }

    /// One fragmented message at a time on the channel: the second starts only after the MAC-END
    /// of the first (23.4.2.1.1, 23.4.3.1.1 i), however many slots the channel has.
    #[test]
    fn test_one_fragmented_message_at_a_time_on_the_channel() {
        let x = TetraAddress::issi(2_145_007);
        let mut sched = multislot_slotter(x.ssi, &[3, 4]);
        for tag in [0xa1u8, 0xb2] {
            let (pdu, sdu) = tagged_resource(&x, tag, 600);
            sched.dl_enqueue_tma_for_link(3, pdu, sdu, None);
        }
        let kinds: Vec<(u8, u8)> = finalize_slots(&mut sched, 24)
            .iter()
            .filter(|s| (3..=4).contains(&s.ts.t))
            .flat_map(dl_pdus)
            .map(|(kind, _, li)| (kind, if kind == 0 { li } else { 0 }))
            .collect();
        assert_eq!(kinds.iter().map(|k| k.0).collect::<Vec<_>>(), vec![0, 2, 3, 0, 2, 3], "{kinds:?}");
        assert!(kinds.iter().filter(|k| k.0 == 0).all(|k| k.1 == 0b111111));
    }

    /// A group PDU copied to the channel of a member goes out once, on one of its slots.
    #[test]
    fn test_a_group_copy_goes_once_on_a_multislot_channel() {
        let x = TetraAddress::issi(2_145_007);
        let group = TetraAddress::new(91, SsiType::Gssi);
        let mut sched = multislot_slotter(x.ssi, &[3, 4]);
        let (pdu, sdu) = tagged_resource(&group, 0x5a, 64);
        sched.dl_enqueue_tma_for_link(4, pdu, sdu, None);
        let slots = finalize_slots(&mut sched, 16);
        let copies: Vec<u8> = slots
            .iter()
            .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(group.ssi)))
            .map(|s| s.ts.t)
            .collect();
        assert_eq!(copies.len(), 1, "{copies:?}");
        assert!((3..=4).contains(&copies[0]));
    }

    /// A PDU queued outside a build for "the next slot" (meant for ts1) is not taken by a slot of
    /// a packet-data channel of several slots built next: it goes on ts1.
    #[test]
    fn test_out_of_build_items_go_to_ts1() {
        let x = TetraAddress::issi(2_145_007);
        let other = TetraAddress::issi(2_260_619);
        let mut sched = multislot_slotter(x.ssi, &[2, 3]);
        // The slot built next is (3,2).
        sched.set_dl_time(TdmaTime { t: 4, f: 2, m: 1, h: 0 });
        let (pdu, sdu) = tagged_resource(&other, 0x77, 32);
        sched.dl_enqueue_tma_next_frame(pdu, sdu, None);
        let slots = finalize_slots(&mut sched, 8);
        assert_eq!(slots[0].ts.t, 2, "a channel slot is built next");
        let on: Vec<u8> = slots
            .iter()
            .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(other.ssi)))
            .map(|s| s.ts.t)
            .collect();
        assert_eq!(on, vec![1]);
    }

    /// A second channel allocation for the MCCH in one frame, held back for the next frame, goes
    /// on ts1 of the next frame, not on the slot built next (ts3 here), where no idle radio
    /// listens; and it does not get stuck there when a call takes that slot (field, 2026-10-03:
    /// a D-SETUP stuck on ts3 for 2.9 s, "queued signaling on ts 3 but no stealing item").
    #[test]
    fn test_a_held_back_chan_alloc_goes_on_the_next_mcch() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        for call_takes_ts3 in [false, true] {
            let mut sched = get_testing_slotter();
            // The slot built next is (2,3).
            sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
            let carrier_num = sched.carrier_num;
            let setup = |gssi: u32| {
                let (mut pdu, sdu) = tagged_resource(&TetraAddress::new(gssi, SsiType::Gssi), 0x08, 78);
                pdu.chan_alloc_element = Some(ChanAllocElement {
                    alloc_type: ChanAllocType::Replace,
                    ts_assigned: [false, true, false, false],
                    ul_dl_assigned: UlDlAssignment::Both,
                    clch_permission: false,
                    cell_change_flag: false,
                    carrier_num,
                    ext: None,
                    mon_pattern: 1,
                    frame18_mon_pattern: None,
                });
                (pdu, sdu)
            };
            let (pdu, sdu) = setup(214_900);
            sched.dl_enqueue_tma(pdu, sdu, None);
            let (pdu, sdu) = setup(214_901);
            sched.dl_enqueue_tma(pdu, sdu, None);
            assert_eq!(sched.dltx_next_slot_queue.len(), 1, "the second is held back");
            let mut slots = finalize_slots(&mut sched, 1);
            assert_eq!((slots[0].ts.f, slots[0].ts.t), (2, 3));
            if call_takes_ts3 {
                sched.create_circuit(Direction::Dl, test_circuit(Direction::Dl, 3));
            }
            slots.extend(finalize_slots(&mut sched, 12));
            let on: Vec<(u8, u8)> = slots
                .iter()
                .filter(|s| dl_pdus(s).iter().any(|p| p.1 == Some(214_901)))
                .map(|s| (s.ts.f, s.ts.t))
                .collect();
            assert_eq!(on, vec![(4, 1)], "call on ts3: {call_takes_ts3}");
            assert!(sched.dltx_queues[2].is_empty(), "call on ts3: {call_takes_ts3}");
        }
    }

    /// A channel allocation that goes whole in an empty block does not open a fragmentation where
    /// it does not fit (the allocation would go in the MAC-RESOURCE, EN 300 392-2 23.4.2.1.1 puts it
    /// in the MAC-END); it waits for the next slot, first in line. Cases: one held back for the
    /// next frame meets the first of that frame (two D-SETUPs of LI=19 do not fit in one SCH/F);
    /// frame 18, which carries none, between two windows; signalling queued ahead of a D-SETUP
    /// (field, 2026-10-03 17:04:42.780: a D-SETUP fragmented behind BL-ACKs).
    #[test]
    fn test_a_channel_allocation_does_not_open_a_fragmentation() {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let setup = |sched: &mut BsChannelScheduler, gssi: u32| {
            let (mut pdu, sdu) = tagged_resource(&TetraAddress::new(gssi, SsiType::Gssi), 0x08, 78);
            pdu.chan_alloc_element = Some(ChanAllocElement {
                alloc_type: ChanAllocType::Replace,
                ts_assigned: [false, true, false, false],
                ul_dl_assigned: UlDlAssignment::Both,
                clch_permission: false,
                cell_change_flag: false,
                carrier_num: sched.carrier_num,
                ext: None,
                mon_pattern: 1,
                frame18_mon_pattern: None,
            });
            sched.dl_enqueue_tma(pdu, sdu, None);
        };
        let setups = [214_900, 214_901, 1_010_269];
        // Where each D-SETUP went whole; fails on one that opened a fragmentation.
        let sent = |slots: &[TmvUnitdataReqSlot]| -> Vec<(u32, u8, u8)> {
            let mut on = Vec::new();
            for s in slots {
                for (kind, ssi, li) in dl_pdus(s) {
                    let Some(ssi) = ssi.filter(|ssi| kind == 0 && setups.contains(ssi)) else {
                        continue;
                    };
                    assert_ne!(li, 0b111111, "{ssi} opened a fragmentation on ({},{})", s.ts.f, s.ts.t);
                    on.push((ssi, s.ts.f, s.ts.t));
                }
            }
            on
        };

        // Held back, then the first of the next frame.
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        setup(&mut sched, 214_900);
        setup(&mut sched, 214_901);
        let mut slots = finalize_slots(&mut sched, 3);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        setup(&mut sched, 1_010_269);
        slots.extend(finalize_slots(&mut sched, 12));
        assert_eq!(sent(&slots), vec![(214_900, 3, 1), (214_901, 4, 1), (1_010_269, 5, 1)]);

        // Frame 18: one waiting from frame 17, two more after the build of (18,1).
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 4, f: 16, m: 1, h: 0 });
        setup(&mut sched, 214_900);
        let mut slots = finalize_slots(&mut sched, 4);
        assert_eq!((slots[3].ts.f, slots[3].ts.t), (18, 1));
        setup(&mut sched, 214_901);
        setup(&mut sched, 1_010_269);
        slots.extend(finalize_slots(&mut sched, 16));
        let mut on: Vec<(u8, u8)> = sent(&slots).iter().map(|s| (s.1, s.2)).collect();
        on.sort();
        assert_eq!(on, vec![(1, 1), (2, 1), (3, 1)], "one a frame from frame 1");

        // Signalling queued ahead leaves 108 bits, room to open a fragmentation but not for the
        // D-SETUP (152 bits).
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, 117);
        sched.dl_enqueue_tma(pdu, sdu, None);
        setup(&mut sched, 214_900);
        let slots = finalize_slots(&mut sched, 8);
        assert_eq!(sent(&slots), vec![(214_900, 4, 1)]);
    }

    /// A resource with a channel allocation to timeslot 2 of this carrier.
    fn chan_alloc_resource(sched: &BsChannelScheduler, addr: TetraAddress, bits: usize) -> (MacResource, BitBuffer) {
        use tetra_pdus::umac::fields::channel_allocation::ChanAllocElement;
        use tetra_saps::lcmc::enums::{alloc_type::ChanAllocType, ul_dl_assignment::UlDlAssignment};
        let (mut pdu, sdu) = tagged_resource(&addr, 0x08, bits);
        pdu.chan_alloc_element = Some(ChanAllocElement {
            alloc_type: ChanAllocType::Replace,
            ts_assigned: [false, true, false, false],
            ul_dl_assigned: UlDlAssignment::Both,
            clch_permission: false,
            cell_change_flag: false,
            carrier_num: sched.carrier_num,
            ext: None,
            mon_pattern: 1,
            frame18_mon_pattern: None,
        });
        (pdu, sdu)
    }

    /// The MAC-RESOURCEs of the downlink SCH/F blocks, in order: (frame, SSI, length indication,
    /// slot grant, random-access flag, channel allocation).
    fn dl_resources(slots: &[TmvUnitdataReqSlot]) -> Vec<(u8, u32, u8, bool, bool, bool)> {
        use tetra_pdus::umac::pdus::mac_end_dl::MacEndDl;
        let mut out = Vec::new();
        for s in slots {
            let Some(blk) = s.blk1.as_ref().filter(|b| b.logical_channel == LogicalChannel::SchF) else {
                continue;
            };
            let mut block = blk.mac_block.clone();
            block.seek(0);
            let bits = block.to_bitstr();
            let mut pos = 0;
            while pos + 16 <= bits.len() {
                let mut b = BitBuffer::from_bitstr(&bits[pos..]);
                b.seek(0);
                let li = match b.peek_bits(3) {
                    Some(0..=1) => {
                        let Ok(res) = MacResource::from_bitbuf(&mut b) else { break };
                        let Some(addr) = res.addr else { break };
                        out.push((
                            s.ts.f,
                            addr.ssi,
                            res.length_ind,
                            res.slot_granting_element.is_some(),
                            res.random_access_flag,
                            res.chan_alloc_element.is_some(),
                        ));
                        res.length_ind
                    }
                    Some(3) => {
                        let Ok(end) = MacEndDl::from_bitbuf(&mut b) else { break };
                        end.length_ind
                    }
                    _ => break,
                };
                if li == 0b111111 {
                    break;
                }
                pos += li as usize * 8;
            }
        }
        out
    }

    /// What goes whole behind a channel allocation waiting for room goes in the block, unless it
    /// is for the allocation's address: signalling ahead leaves 60 bits, too few for a D-SETUP of
    /// 152, and a random-access acknowledgement with a grant (56 bits) goes there, on time; a PDU
    /// to the D-SETUP's group that would fit too stays behind it.
    #[test]
    fn test_whole_signalling_goes_past_a_waiting_channel_allocation() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, 165);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let group = TetraAddress::new(214_900, SsiType::Gssi);
        let (pdu, sdu) = chan_alloc_resource(&sched, group, 78);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = tagged_resource(&group, 0x0a, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let other = TetraAddress::issi(2_145_008);
        sched.dl_enqueue_random_access_ack(1, other);
        let (grant, marker) = sched
            .ul_process_cap_req(1, other, &ReservationRequirement::Req1Slot)
            .expect("grant");
        sched.dl_enqueue_grant(1, other, grant, marker);

        let slots = finalize_slots(&mut sched, 12);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let sent = dl_resources(&slots);
        let at = |ssi: u32| -> Vec<(u8, u8, bool)> { sent.iter().filter(|r| r.1 == ssi).map(|r| (r.0, r.2, r.5)).collect() };
        assert_eq!(at(2_145_007), vec![(3, 26, false)]);
        assert_eq!(at(other.ssi), vec![(3, 7, false)], "the acknowledgement and grant on time");
        assert_eq!(
            at(group.ssi),
            vec![(4, 19, true), (4, 7, false)],
            "the D-SETUP whole, then its group's PDU"
        );
    }

    /// A grant merged into a channel allocation that waits for room does not wait with it: it goes
    /// on its own, with the random-access acknowledgement, in the block it was given for. Here
    /// with the grant the D-CONNECT (112 bits) no longer fits in the 116 bits left.
    #[test]
    fn test_a_grant_does_not_wait_with_a_channel_allocation() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, 109);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let x = TetraAddress::issi(1_030_036);
        let (pdu, sdu) = chan_alloc_resource(&sched, x, 44);
        sched.dl_enqueue_tma(pdu, sdu, None);
        sched.dl_enqueue_random_access_ack(1, x);
        let (grant, marker) = sched.ul_process_cap_req(1, x, &ReservationRequirement::Req1Slot).expect("grant");
        sched.dl_enqueue_grant(1, x, grant, marker);

        let slots = finalize_slots(&mut sched, 12);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let x_at: Vec<_> = dl_resources(&slots).into_iter().filter(|r| r.1 == x.ssi).collect();
        // (frame, ssi, li, grant, random access, chan_alloc)
        assert_eq!(x_at, vec![(3, x.ssi, 7, true, true, false), (4, x.ssi, 14, false, false, true)]);
    }

    /// A random-access acknowledgement merged into a waiting channel allocation without a grant
    /// goes now on its own, and the allocation follows without it (WT, 23.5.1.4.8).
    #[test]
    fn test_a_random_access_ack_does_not_wait_with_a_channel_allocation() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, 117);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let x = TetraAddress::issi(1_030_036);
        let (pdu, sdu) = chan_alloc_resource(&sched, x, 44);
        sched.dl_enqueue_tma(pdu, sdu, None);
        sched.dl_enqueue_random_access_ack(1, x);

        let slots = finalize_slots(&mut sched, 12);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let x_at: Vec<_> = dl_resources(&slots)
            .into_iter()
            .filter(|r| r.1 == x.ssi)
            .map(|r| (r.0, r.3, r.4, r.5))
            .collect();
        // (frame, grant, random access, chan_alloc)
        assert_eq!(x_at, vec![(3, false, true, false), (4, false, false, true)]);
    }

    /// Channel allocations go in the order they came, also when one waits for room while those
    /// held back for the next frame join the queue: two calls a window for three frames.
    #[test]
    fn test_channel_allocations_keep_their_order() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        let group = |n: u32| TetraAddress::new(214_900 + n, SsiType::Gssi);
        let mut slots = Vec::new();
        for n in (0..6).step_by(2) {
            for k in 0..2 {
                let (pdu, sdu) = chan_alloc_resource(&sched, group(n + k), 78);
                sched.dl_enqueue_tma(pdu, sdu, None);
            }
            // Up to and including the next build of ts1.
            loop {
                let s = finalize_slots(&mut sched, 1);
                let ts1 = s[0].ts.t == 1;
                slots.extend(s);
                if ts1 {
                    break;
                }
            }
        }
        slots.extend(finalize_slots(&mut sched, 24));
        let sent: Vec<(u32, u8)> = dl_resources(&slots).iter().filter(|r| r.5).map(|r| (r.1 - 214_900, r.2)).collect();
        assert_eq!(sent, (0..6).map(|n| (n, 19)).collect::<Vec<_>>(), "in order and whole");
    }

    /// What goes past a waiting channel allocation keeps the order of every address it passes, not
    /// only the allocation's: behind a waiting D-SETUP to G1 the D-SETUP to G2 does not fit either,
    /// and G2's D-RELEASE queued after it, which would, does not overtake it.
    #[test]
    fn test_nothing_overtakes_a_pdu_for_its_address_behind_a_waiting_channel_allocation() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        // A long PDU ahead opens a fragmentation on (3,1); its MAC-END leaves 100 bits on (4,1).
        let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, 375);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let g1 = TetraAddress::new(214_900, SsiType::Gssi);
        let g2 = TetraAddress::new(214_901, SsiType::Gssi);
        for g in [g1, g2] {
            let (pdu, sdu) = chan_alloc_resource(&sched, g, 78);
            sched.dl_enqueue_tma(pdu, sdu, None);
        }
        assert_eq!(sched.dltx_next_slot_queue.len(), 1, "G2's D-SETUP held back for the next frame");
        let mut slots = finalize_slots(&mut sched, 3);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let (pdu, sdu) = tagged_resource(&g2, 0x0a, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);
        slots.extend(finalize_slots(&mut sched, 16));
        // (frame, chan_alloc)
        let g2_sent: Vec<(u8, bool)> = dl_resources(&slots).iter().filter(|r| r.1 == g2.ssi).map(|r| (r.0, r.5)).collect();
        assert_eq!(g2_sent.len(), 2, "{g2_sent:?}");
        assert!(g2_sent[0].1 && !g2_sent[1].1, "G2's D-SETUP before its D-RELEASE: {g2_sent:?}");
    }

    /// The grant split off a waiting channel allocation carries no usage marker: the one on the
    /// allocation is the call's traffic usage marker, and without the allocation the radio ignores
    /// it (EN 300 392-2 23.5.5). With 52 bits left the grant (51 bits) goes on time; with the
    /// marker (57) it would wait with the allocation, a frame after the slot it grants.
    #[test]
    fn test_a_grant_split_off_a_channel_allocation_has_no_usage_marker() {
        for ahead in [173, 109] {
            let mut sched = get_testing_slotter();
            sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
            let (pdu, sdu) = tagged_resource(&TetraAddress::issi(2_145_007), 0x09, ahead);
            sched.dl_enqueue_tma(pdu, sdu, None);
            let x = TetraAddress::issi(1_030_036);
            let (mut pdu, sdu) = chan_alloc_resource(&sched, x, 44);
            pdu.usage_marker = Some(10);
            sched.dl_enqueue_tma(pdu, sdu, None);
            sched.dl_enqueue_random_access_ack(1, x);
            let (grant, marker) = sched.ul_process_cap_req(1, x, &ReservationRequirement::Req1Slot).expect("grant");
            sched.dl_enqueue_grant(1, x, grant, marker);
            let slots = finalize_slots(&mut sched, 12);
            assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
            let x_at: Vec<_> = dl_resources(&slots).into_iter().filter(|r| r.1 == x.ssi && r.3).collect();
            // (frame, ssi, li, grant, random access, chan_alloc): 7 octets, no usage marker.
            assert_eq!(x_at, vec![(3, x.ssi, 7, true, true, false)], "{ahead} bits ahead");
        }
    }

    /// `chan_alloc_resource` sending the radio to `carrier`.
    fn chan_alloc_to(sched: &BsChannelScheduler, addr: TetraAddress, bits: usize, carrier: u16) -> (MacResource, BitBuffer) {
        let (mut pdu, sdu) = chan_alloc_resource(sched, addr, bits);
        if let Some(a) = pdu.chan_alloc_element.as_mut() {
            a.carrier_num = carrier;
        }
        (pdu, sdu)
    }

    /// The MAC-RESOURCEs of `slots`, in order: (slot time, SSI, channel allocation).
    fn dl_resources_at(slots: &[TmvUnitdataReqSlot]) -> Vec<(TdmaTime, u32, bool)> {
        slots
            .iter()
            .flat_map(|s| dl_resources(std::slice::from_ref(s)).into_iter().map(move |r| (s.ts, r.1, r.5)))
            .collect()
    }

    /// A channel allocation to a carrier warming up (`secondary_carrier_on_demand`) waits for it,
    /// however long: whole signalling for another address goes past it, but neither a PDU for its
    /// address nor a second allocation to that carrier. Once the carrier is on the air it goes on
    /// the next ts1 that is not frame 18, the others behind it in their order.
    #[test]
    fn test_a_channel_allocation_waits_for_its_carrier_to_warm_up() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.set_cold_carriers(&[1002]);
        let (x, y, z) = (
            TetraAddress::issi(1_030_036),
            TetraAddress::issi(2_145_007),
            TetraAddress::issi(2_145_008),
        );
        let (pdu, sdu) = chan_alloc_to(&sched, x, 44, 1002);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = tagged_resource(&y, 0x09, 40);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = tagged_resource(&x, 0x0a, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = chan_alloc_to(&sched, z, 44, 1002);
        sched.dl_enqueue_tma(pdu, sdu, None);

        let slots = finalize_slots(&mut sched, 3 * 72);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let sent = dl_resources_at(&slots);
        let at = |sent: &[(TdmaTime, u32, bool)], ssi: u32| -> Vec<(TdmaTime, bool)> {
            sent.iter().filter(|r| r.1 == ssi).map(|r| (r.0, r.2)).collect()
        };
        assert_eq!(at(&sent, y.ssi), vec![(slots[2].ts, false)], "another address goes past it");
        assert!(
            at(&sent, x.ssi).is_empty(),
            "neither the allocation nor its address's PDU: {sent:?}"
        );
        assert!(at(&sent, z.ssi).is_empty(), "nor a second allocation to that carrier: {sent:?}");
        assert_eq!(sched.allocation_carriers().collect::<Vec<_>>(), vec![1002, 1002]);

        // On the air: the next ts1 that is not frame 18.
        sched.set_cold_carriers(&[]);
        let slots = finalize_slots(&mut sched, 3 * 72);
        let first_mcch = slots.iter().find(|s| s.ts.t == 1 && s.ts.f != 18).map(|s| s.ts).unwrap();
        let sent = dl_resources_at(&slots);
        let x_at = at(&sent, x.ssi);
        assert_eq!(x_at.first(), Some(&(first_mcch, true)), "{sent:?}");
        assert_eq!(x_at.len(), 2, "then its PDU: {sent:?}");
        let z_at = at(&sent, z.ssi);
        assert_eq!(z_at.len(), 1, "{sent:?}");
        assert!(
            z_at[0].1 && z_at[0].0.diff(x_at[0].0) > 0,
            "the second allocation after the first: {sent:?}"
        );
        assert_eq!(sched.allocation_carriers().count(), 0);
    }

    /// The grant and random-access acknowledgement merged into a channel allocation to a carrier
    /// warming up go on time on their own, without a usage marker (as for one waiting for room).
    #[test]
    fn test_a_grant_does_not_wait_with_an_allocation_to_a_cold_carrier() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.set_cold_carriers(&[1002]);
        let x = TetraAddress::issi(1_030_036);
        let (mut pdu, sdu) = chan_alloc_to(&sched, x, 44, 1002);
        pdu.usage_marker = Some(10);
        sched.dl_enqueue_tma(pdu, sdu, None);
        sched.dl_enqueue_random_access_ack(1, x);
        let (grant, marker) = sched.ul_process_cap_req(1, x, &ReservationRequirement::Req1Slot).expect("grant");
        sched.dl_enqueue_grant(1, x, grant, marker);
        let slots = finalize_slots(&mut sched, 12);
        assert_eq!((slots[2].ts.f, slots[2].ts.t), (3, 1));
        let x_at: Vec<_> = dl_resources(&slots).into_iter().filter(|r| r.1 == x.ssi).collect();
        // (frame, ssi, li, grant, random access, chan_alloc): 7 octets, no usage marker.
        assert_eq!(x_at, vec![(3, x.ssi, 7, true, true, false)]);
    }

    /// Only an allocation to a carrier warming up waits: one to another carrier, one to that
    /// carrier once it is on the air, and a quit (no timeslot) go on the next MCCH.
    #[test]
    fn test_only_an_allocation_to_a_cold_carrier_waits() {
        for (carrier, cold, quit) in [(1001, true, false), (1002, false, false), (1002, true, true)] {
            let mut sched = get_testing_slotter();
            sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
            if cold {
                sched.set_cold_carriers(&[1002]);
            }
            let x = TetraAddress::issi(1_030_036);
            let (mut pdu, sdu) = chan_alloc_to(&sched, x, 44, carrier);
            if quit && let Some(a) = pdu.chan_alloc_element.as_mut() {
                a.ts_assigned = [false; 4];
            }
            sched.dl_enqueue_tma(pdu, sdu, None);
            let slots = finalize_slots(&mut sched, 12);
            let x_at: Vec<_> = dl_resources(&slots)
                .into_iter()
                .filter(|r| r.1 == x.ssi)
                .map(|r| (r.0, r.5))
                .collect();
            assert_eq!(x_at, vec![(3, true)], "carrier {carrier}, cold {cold}, quit {quit}");
        }
    }

    /// `allocation_carriers` lists the carriers queued channel allocations send radios to: a
    /// resource, a fragmented message not started yet and one held back for the next frame; not a
    /// quit or a PDU without an allocation.
    #[test]
    fn test_allocation_carriers_lists_the_queued_allocations() {
        let mut sched = get_testing_slotter();
        let addr = |n: u32| TetraAddress::issi(1_030_030 + n);
        let (pdu, sdu) = chan_alloc_to(&sched, addr(1), 44, 1002);
        sched.dltx_queues[0].push(DlSchedElem::Resource(pdu, sdu, None, false));
        let (pdu, sdu) = chan_alloc_to(&sched, addr(2), 44, 1003);
        sched.dltx_queues[1].push(DlSchedElem::FragBuf(BsFragger::new(pdu, sdu, None)));
        let (mut pdu, sdu) = chan_alloc_to(&sched, addr(3), 44, 1004);
        if let Some(a) = pdu.chan_alloc_element.as_mut() {
            a.ts_assigned = [false; 4];
        }
        sched.dltx_queues[0].push(DlSchedElem::Resource(pdu, sdu, None, false));
        let (pdu, sdu) = tagged_resource(&addr(4), 0x09, 40);
        sched.dltx_queues[0].push(DlSchedElem::Resource(pdu, sdu, None, false));
        let (pdu, sdu) = chan_alloc_to(&sched, addr(5), 44, 1005);
        sched.dltx_next_slot_queue.push(DlSchedElem::Resource(pdu, sdu, None, false));
        assert_eq!(sched.allocation_carriers().collect::<Vec<_>>(), vec![1002, 1003, 1005]);
    }

    /// Frame 18 already leaves a channel of three slots three slots without a fragment, and N.203
    /// is only ">= 4" (annex B.2): next to it (frames 17 and 1) signalling for the radio does not go
    /// ahead of the fragment under way on its channel, the fragment opens the block.
    #[test]
    fn test_a_fragment_is_not_delayed_next_to_frame_18() {
        let x = TetraAddress::issi(2_145_007);
        let mut sched = multislot_slotter(x.ssi, &[2, 3, 4]);
        // The slot built next is (16,2).
        sched.set_dl_time(TdmaTime { t: 4, f: 15, m: 1, h: 0 });

        let reporter = TxReporter::new();
        let (pdu, sdu) = tagged_resource(&x, 0xd0, 3200);
        sched.dl_enqueue_packet_data_for_link(4, pdu, sdu, Some(reporter.clone()));
        let mut slots = finalize_slots(&mut sched, 3);
        assert_eq!(dl_pdus(&slots[0]).first().map(|p| p.0), Some(0), "the datagram starts on (16,2)");
        let (sig, sdu) = tagged_resource(&x, 0x51, 40);
        sched.dl_enqueue_tma_for_link(4, sig, sdu, None);
        slots.extend(finalize_slots(&mut sched, 4 * 5));
        for s in slots.iter().filter(|s| s.ts.t >= 2 && matches!(s.ts.f, 17 | 1)) {
            let first = dl_pdus(s).first().map(|p| p.0);
            assert!(
                matches!(first, Some(2 | 3)),
                "a fragment opens ({},{}): {:?}",
                s.ts.f,
                s.ts.t,
                dl_pdus(s)
            );
        }
        assert!(reporter.is_transmitted(), "the datagram completed");
        let signalling = slots
            .iter()
            .filter(|s| dl_pdus(s).iter().any(|p| p.0 == 0 && p.2 != 0b111111))
            .count();
        assert!(signalling >= 1, "and the signalling went after it");
    }

    /// Whole signalling goes ahead of a packet-data TM-SDU being fragmented on the MCCH, but not a
    /// channel allocation to a carrier warming up, nor a PDU for its address behind it; the grant
    /// and random-access acknowledgement merged into it go on their own. Once the carrier is on the
    /// air the allocation goes, then that PDU.
    #[test]
    fn test_a_cold_allocation_does_not_pass_a_packet_data_fragment() {
        let mut sched = get_testing_slotter();
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.set_cold_carriers(&[1002]);
        let (x, y, z) = (
            TetraAddress::issi(1_030_036),
            TetraAddress::issi(2_145_007),
            TetraAddress::issi(2_145_008),
        );
        let reporter = TxReporter::new();
        let (pdu, sdu) = tagged_resource(&y, 0xd0, 6400);
        sched.dl_enqueue_packet_data_for_link(0, pdu, sdu, Some(reporter.clone()));
        finalize_slots(&mut sched, 4);
        let (pdu, sdu) = chan_alloc_to(&sched, x, 44, 1002);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = tagged_resource(&x, 0x0a, 8);
        sched.dl_enqueue_tma(pdu, sdu, None);
        let (pdu, sdu) = tagged_resource(&z, 0x09, 40);
        sched.dl_enqueue_tma(pdu, sdu, None);
        sched.dl_enqueue_random_access_ack(1, x);
        let (grant, marker) = sched.ul_process_cap_req(1, x, &ReservationRequirement::Req1Slot).expect("grant");
        sched.dl_enqueue_grant(1, x, grant, marker);

        let slots = finalize_slots(&mut sched, 4 * 8);
        assert!(!reporter.is_transmitted(), "the datagram is still under way");
        let sent = dl_resources(&slots);
        let x_at: Vec<_> = sent.iter().filter(|r| r.1 == x.ssi).collect();
        // (frame, ssi, li, grant, random access, chan_alloc): 7 octets, no usage marker.
        assert_eq!(x_at, vec![&(4, x.ssi, 7, true, true, false)], "{sent:?}");
        assert_eq!(sent.iter().filter(|r| r.1 == z.ssi).count(), 1, "another address goes: {sent:?}");

        sched.set_cold_carriers(&[]);
        let slots = finalize_slots(&mut sched, 4 * 8);
        assert!(!reporter.is_transmitted(), "still under way");
        let x_at: Vec<_> = dl_resources(&slots).into_iter().filter(|r| r.1 == x.ssi).map(|r| r.5).collect();
        assert_eq!(x_at, vec![true, false], "the allocation, then the PDU");
    }

    /// On a packet-data channel of several slots, group signalling copied there goes ahead of the
    /// radio's fragment under way, but not a channel allocation to a carrier warming up.
    #[test]
    fn test_a_cold_group_allocation_does_not_pass_a_fragment_on_a_pdch() {
        let y = TetraAddress::issi(2_145_007);
        let g = TetraAddress::new(214_900, SsiType::Gssi);
        let mut sched = multislot_slotter(y.ssi, &[2, 3, 4]);
        sched.set_dl_time(TdmaTime { t: 1, f: 2, m: 1, h: 0 });
        sched.set_cold_carriers(&[1002]);
        let reporter = TxReporter::new();
        let (pdu, sdu) = tagged_resource(&y, 0xd0, 12800);
        sched.dl_enqueue_packet_data_for_link(2, pdu, sdu, Some(reporter.clone()));
        finalize_slots(&mut sched, 4);
        let (pdu, sdu) = chan_alloc_to(&sched, g, 44, 1002);
        sched.dl_enqueue_tma_for_link(2, pdu, sdu, None);
        let slots = finalize_slots(&mut sched, 4 * 4);
        assert!(!reporter.is_transmitted(), "the datagram is still under way");
        let g_at = dl_resources_at(&slots).into_iter().filter(|r| r.1 == g.ssi).count();
        assert_eq!(g_at, 0, "held while the carrier warms up");

        sched.set_cold_carriers(&[]);
        let slots = finalize_slots(&mut sched, 4 * 4);
        assert!(!reporter.is_transmitted(), "still under way");
        let g_at: Vec<_> = dl_resources_at(&slots).into_iter().filter(|r| r.1 == g.ssi).collect();
        assert!(
            g_at.len() == 1 && g_at[0].2 && (2..=4).contains(&g_at[0].0.t),
            "then on the channel: {g_at:?}"
        );
    }
}
