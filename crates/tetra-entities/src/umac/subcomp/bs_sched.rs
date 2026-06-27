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
    umac::subcomp::{bs_frag::BsFragger, circuit_mgr::CircuitMgr},
};

/// We submit this many TX timeslots ahead of the current time
pub const MACSCHED_TX_AHEAD: usize = 1;

// We schedule up to this many frames ahead
pub const MACSCHED_NUM_FRAMES: usize = 18;

const NULL_PDU_LEN_BITS: usize = 16;

pub const SCH_HD_CAP: usize = 124;
pub const SCH_F_CAP: usize = 268;
pub const TCH_S_CAP: usize = 274;

/// Number of timeslots the scheduler operates on. May become larger when secondary carriers are supported.
pub const NUM_TIMESLOTS: usize = 4;

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

    /// Per-timeslot set of SSIs whose RandomAccessAck was dropped by dl_drop_all_except_stolen.
    /// The next STCH built for a matching SSI should carry random_access_flag=true to properly
    /// acknowledge the random access per ETSI 21.4.3.1.
    pending_ra_acks: [Vec<u32>; 4],

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
    Resource(MacResource, BitBuffer, Option<TxReporter>),

    /// A FragBuf containing remaining non-transmitted information after a MAC-RESOURCE start has been transmitted
    FragBuf(BsFragger),

    /// Pre-built STCH block for FACCH/stealing a half-slot from traffic channel.
    /// Contains MAC-U-SIGNAL (3 bits) + TM-SDU = 124 type1 bits.
    /// Delivers time-critical signaling (D-TX CEASED, D-TX GRANTED) per EN 300 392-2, clause 23.5.
    Stealing(BitBuffer, Option<TxReporter>),
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
            pending_ra_acks: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            mcch_chan_alloc_sent_this_frame: false,
            // Start each timeslot's marker cursor at 4 (first valid value).
            next_usage_marker: [4, 4, 4, 4],
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

    pub fn allow_mcch(&self) -> bool {
        self.downlink_mode.allow_mcch()
    }

    pub fn allow_common_control_aach(&self) -> bool {
        self.downlink_mode.allow_common_control_aach()
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

    fn supports_assigned_traffic_ts(&self, ts: u8) -> bool {
        match ts {
            1 => self.downlink_mode.allow_assigned_traffic_ts1(),
            2..=4 => true,
            _ => false,
        }
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

            if candidate_t.is_mandatory_clch() {
                // Not an opportunity; skip
                continue;
            }

            if candidate_t.f == 18 {
                // Skip frame 18 — ACCESS-ASSIGN marks UL as CommonOnly on this frame,
                // and timing at the multiframe boundary causes grant delivery to fail.
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
        if ts == 1 && !self.allow_mcch() && !self.circuits.is_active(Direction::Dl, self.carrier_num, ts) {
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
        self.dltx_queues[ts as usize - 1].push(elem);
    }

    pub fn dl_enqueue_random_access_ack(&mut self, ts: u8, addr: TetraAddress) {
        if ts == 1 && !self.allow_mcch() && !self.circuits.is_active(Direction::Dl, self.carrier_num, ts) {
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
        self.dltx_queues[ts as usize - 1].push(elem);
    }

    fn identify_timeslots_for_ssi(&self, addr: Option<TetraAddress>, link_id: LinkId) -> [u8; NUM_TIMESLOTS] {
        let Some(addr) = addr else {
            tracing::warn!("identify_timeslots_for_ssi: MAC-RESOURCE has no address, dropping");
            return [0, 0, 0, 0];
        };

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

        if !self.allow_mcch() && link_ts == 1 {
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
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter);
                self.dltx_next_slot_queue.push(elem);
                break;
            } else if next_ts > 0 {
                // There is another ts for which we need to transmit this message.
                // Clone the message now and push it to the current ts.
                let elem = DlSchedElem::Resource(pdu.clone(), sdu.clone(), tx_reporter.clone());
                self.dltx_queues[ts as usize - 1].push(elem);
            } else {
                // This is the last ts on which we need to transmit this message
                let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter);
                self.dltx_queues[ts as usize - 1].push(elem);
                break;
            }
        }
    }

    pub fn dl_enqueue_tma(&mut self, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let timeslots = self.identify_timeslots_for_ssi(pdu.addr, 0);
        self.dl_enqueue_tma_on_timeslots(timeslots, pdu, sdu, tx_reporter);
    }

    pub fn dl_enqueue_tma_for_link(&mut self, link_id: LinkId, pdu: MacResource, sdu: BitBuffer, tx_reporter: Option<TxReporter>) {
        let timeslots = self.identify_timeslots_for_ssi(pdu.addr, link_id);
        self.dl_enqueue_tma_on_timeslots(timeslots, pdu, sdu, tx_reporter);
    }

    /// Consumes and returns true if a pending random access ack exists for the given SSI on
    /// this timeslot. Used when building STCH blocks so the MAC-RESOURCE can carry
    /// random_access_flag=true per ETSI 21.4.3.1.
    pub fn take_pending_ra_ack(&mut self, ts: u8, ssi: u32) -> bool {
        let pending = &mut self.pending_ra_acks[ts as usize - 1];
        if let Some(pos) = pending.iter().position(|&s| s == ssi) {
            pending.remove(pos);
            true
        } else {
            false
        }
    }

    /// Enqueue a pre-built STCH block for FACCH/stealing on a traffic timeslot.
    /// The block must be 124 type1 bits containing MAC-U-SIGNAL header + TM-SDU.
    pub fn dl_enqueue_stealing(&mut self, ts: u8, block: BitBuffer, tx_reporter: Option<TxReporter>) {
        tracing::info!("dl_enqueue_stealing: ts {} enqueueing STCH block ({} bits)", ts, block.get_len());
        self.dltx_queues[ts as usize - 1].push(DlSchedElem::Stealing(block, tx_reporter));
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
        let elem = DlSchedElem::Resource(pdu, sdu, tx_reporter);
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
        let queue = &mut self.dltx_queues[ts.t as usize - 1];

        for index in 0..queue.len() {
            let elem = &mut queue[index];
            if let DlSchedElem::Resource(pdu, _sdu, _repeat) = elem {
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
        let queue = &mut self.dltx_queues[timeslot as usize - 1];
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
                    DlSchedElem::Resource(_, _, tx_reporter) => {
                        // Report as discarded manually
                        if let Some(tx_reporter) = tx_reporter {
                            tx_reporter.mark_discarded();
                        }
                    }

                    DlSchedElem::FragBuf(_) => {
                        // Fragger self-marks any unsent fragments as discarded when dropped, so we don't need to do anything here.
                    }

                    DlSchedElem::RandomAccessAck(addr) => {
                        // Save the SSI so the next STCH for this address can carry
                        // random_access_flag=true (ETSI 21.4.3.1)
                        self.pending_ra_acks[timeslot as usize - 1].push(addr.ssi);
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
            let mac_resource = self.dl_get_scheduled_resource_for_ssi(ts, addr);
            match mac_resource {
                Some(DlSchedElem::Resource(pdu, _sdu, _repeat)) => {
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
                    let dlsched_res = DlSchedElem::Resource(pdu, BitBuffer::new(0), None);
                    self.dltx_queues[ts.t as usize - 1].push(dlsched_res);
                }
                _ => unreachable!("BUG: unhandled match variant -- should never be reached"),
            }
        }
    }

    fn dl_build_block_from_signalling_schedule(&mut self, ts: TdmaTime) -> Option<BitBuffer> {
        let mut buf_opt = None;

        while !self.dltx_queues[ts.t as usize - 1].is_empty() {
            let opt = self.dl_take_prioritized_sched_item(ts);

            match opt {
                Some(sched_elem) => {
                    match sched_elem {
                        DlSchedElem::Broadcast(_) => {
                            unimplemented_log!("finalize_ts_for_tick: Broadcast scheduling not implemented");
                        }

                        DlSchedElem::Resource(pdu, sdu, tx_reporter) => {
                            // Allocate bitbuf if not already done
                            let mut buf = buf_opt.unwrap_or_else(|| BitBuffer::new(SCH_F_CAP));
                            // Create fragger, either to send the whole PDU or to start fragmentation
                            let mut fragger = BsFragger::new(pdu, sdu, tx_reporter);
                            if !fragger.get_next_chunk(&mut buf) {
                                // Fragmentation was started and we have more chunks to send
                                // Enqueue fragger with remaining data for retrieval next frame
                                self.dl_enqueue_tma_frag_next_frame(fragger);
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
                            }
                            buf_opt = Some(buf);
                        }

                        DlSchedElem::Stealing(_, tx_reporter) => {
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
        if !self.dltx_next_slot_queue.is_empty() {
            let current = &mut self.dltx_queues[ts.t as usize - 1];
            // Prepend: move deferred items to front, then re-append any items already queued.
            let mut merged = std::mem::take(&mut self.dltx_next_slot_queue);
            merged.extend(current.drain(..));
            *current = merged;
        }

        buf_opt
    }

    /// Build traffic block for active circuit. Returns (tch_block, optional_stch_block):
    /// - tch_block: speech/silence (274 bits)
    /// - stch_block: STCH signaling (124 bits) for FACCH stealing (EN 300 392-2, clause 23.5)
    /// Also reports transmission, if a TxReporter was attached to the DlSchedElem::Stealing element
    fn dl_build_traffic_block(&mut self, ts: TdmaTime) -> (BitBuffer, Option<BitBuffer>) {
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

        // Check for FACCH/stealing: take a queued Stealing item (highest priority signaling)
        let (stch_opt, tx_reporter_opt) = {
            let q = &mut self.dltx_queues[ts.t as usize - 1];
            if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::Stealing(..))) {
                match q.remove(i) {
                    DlSchedElem::Stealing(buf, tx_reporter) => (Some(buf), tx_reporter),
                    _ => unreachable!(),
                }
            } else {
                (None, None)
            }
        };

        // Warn about other queued signaling that can't be sent via stealing yet
        if stch_opt.is_none() && !self.dltx_queues[ts.t as usize - 1].is_empty() {
            tracing::warn!("dl_build_traffic_block: queued signaling on ts {} but no stealing item", ts.t);
        }

        // If desired, report transmission
        if let Some(tx_reporter) = tx_reporter_opt {
            tx_reporter.mark_transmitted();
        }

        (tch_buf, stch_opt)
    }

    /// Return first queued grant.
    /// If none; return first in-progress fragmented message.
    /// If none; return first to-be-transmitted resource.
    /// If none, return None.
    pub fn dl_take_prioritized_sched_item(&mut self, ts: TdmaTime) -> Option<DlSchedElem> {
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
        let slot = ts.t as usize - 1;
        let Some(q) = self.dltx_queues.get_mut(slot) else {
            return None;
        };

        // Return grants first
        if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::Grant(..))) {
            return Some(q.remove(i));
        }

        // Return FragBufs next
        if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::FragBuf(_))) {
            return Some(q.remove(i));
        }

        // Return Resources next
        if let Some(i) = q.iter().position(|e| matches!(e, DlSchedElem::Resource(_, _, _))) {
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
            let (tch_buf, stch_opt) = self.dl_build_traffic_block(ts);

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
                    blk2: Some(TmvUnitdataReq {
                        logical_channel: LogicalChannel::TchS,
                        mac_block: tch_buf,
                        scrambling_code: self.scrambling_code,
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
            self.dl_integrate_sched_elems_for_timeslot(ts);

            let buf = self.dl_build_block_from_signalling_schedule(ts);
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
                1 => {
                    if self.allow_common_control_aach() {
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
                    } else {
                        let in_hangtime = self.supports_assigned_traffic_ts(ts.t) && self.hangtime[ts.t as usize - 1];

                        if in_hangtime && (dl_traffic_usage.is_some() || ul_traffic_usage.is_some()) {
                            aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                            aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                            aach.f2_af = Some(AccessField {
                                access_code: 0,
                                base_frame_len: 4,
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
                }
                2..=4 => {
                    // Additional channels (TS2..TS4).
                    // Normal operation: Traffic(usage) when a circuit is active, else Unallocated.
                    // Hangtime: immediately switch AACH to AssignedControl so radios
                    // detect the end of traffic in the same frame as D-TX CEASED.
                    // The timeslot may still be in traffic mode (for STCH delivery) but
                    // the AACH reflects the new channel state.
                    let in_hangtime = (2..=4).contains(&ts.t) && self.hangtime[ts.t as usize - 1];

                    if in_hangtime && (dl_traffic_usage.is_some() || ul_traffic_usage.is_some()) {
                        aach.dl_usage = AccessAssignDlUsage::AssignedControl;
                        // AssignedOnly (Header 2) allows random access for MSs on
                        // the assigned channel while blocking common control MSs.
                        aach.ul_usage = AccessAssignUlUsage::AssignedOnly;
                        aach.f2_af = Some(AccessField {
                            access_code: 0,
                            base_frame_len: 4,
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
        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None);

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
        sched.dl_enqueue_stealing(1, BitBuffer::new(124), None);

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

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None);
        assert!(sched.has_pending_stealing(2));

        assert!(
            sched
                .dl_build_block_from_signalling_schedule(TdmaTime { t: 2, f: 1, m: 1, h: 0 })
                .is_none()
        );
        assert!(!sched.has_pending_stealing(2));
    }

    #[test]
    fn test_frame_18_keeps_stealing_queued() {
        let mut sched = get_testing_slotter();

        sched.dl_enqueue_stealing(2, BitBuffer::new(124), None);
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
}
